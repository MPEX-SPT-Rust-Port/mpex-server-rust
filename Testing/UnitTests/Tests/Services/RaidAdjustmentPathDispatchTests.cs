using System.Reflection;
using NUnit.Framework;
using SPTarkov.Common.Models.Logging;
using SPTarkov.Server.Core.Generators;
using SPTarkov.Server.Core.Generators.Bot;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Helpers;
using SPTarkov.Server.Core.Helpers.Commerce;
using SPTarkov.Server.Core.Helpers.InRaid;
using SPTarkov.Server.Core.Helpers.Profile;
using SPTarkov.Server.Core.Helpers.Quest;
using SPTarkov.Server.Core.Helpers.Traders;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common;
using SPTarkov.Server.Core.Models.Eft.Game;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Location;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native.Raid;
using SPTarkov.Server.Core.Servers;
using SPTarkov.Server.Core.Services.Bot;
using SPTarkov.Server.Core.Services.Commerce;
using SPTarkov.Server.Core.Services.InRaid;
using SPTarkov.Server.Core.Services.Locales;
using SPTarkov.Server.Core.Services.Profile;
using SPTarkov.Server.Core.Utils;
using SPTarkov.Server.Core.Utils.Cloners;

namespace UnitTests.Tests.Services;

/// <summary>
/// Pins the dual-path dispatch for raid setup: native by default, the retained C# implementations
/// when <c>LocationConfig.ForceLegacyRaidAdjustments</c> is set, when the frozen constructor built the
/// instance (no native seam to dispatch to), or when a mod substituted the service itself. A live
/// Harmony patch is the fourth reason, and <see cref="RaidAdjustmentHookLivenessTests"/> covers the
/// whole frozen set.
///
/// The two scopes are deliberately different and both are pinned here: the patch check is
/// family-wide - one shared set, all four call sites - while the substituted-service check is
/// per-service, so a replaced <see cref="RaidTimeAdjustmentService"/> falls back at its own two call
/// sites and leaves the lifecycle service on the native path.
///
/// Mutates the shared <see cref="LocationConfig"/> singleton's force flag, restored per case, and
/// never runs in parallel. Every pass works on a clone, which is what the real pipeline hands it, so
/// the resident location table is never written to.
/// </summary>
[TestFixture]
[NonParallelizable]
public class RaidAdjustmentPathDispatchTests
{
    /// <summary>
    /// The map every case runs against, cloned per call.
    /// </summary>
    private const string RaidMap = "bigmap";

    /// <summary>
    /// The extract pass only adjusts for a scav; nothing here depends on what it adjusted, but the
    /// dispatch check it makes is the same on either side.
    /// </summary>
    private const string ScavSide = "Savage";

    /// <summary>
    /// The raid time request side. A pmc returns before any draw or settings lookup, and the path is
    /// picked ahead of that - so this is the cheapest request that still dispatches.
    /// </summary>
    private const string PmcSide = "pmc";

    /// <summary>
    /// Both lifecycle passes are <c>protected</c>, and a subclass that exposed them would flip the
    /// path predicate to legacy - so they are called by reflection, on whichever instance is under
    /// test.
    /// </summary>
    private static readonly MethodInfo _adjustExtracts = typeof(LocationLifecycleService).GetMethod(
        "AdjustExtracts",
        BindingFlags.Instance | BindingFlags.NonPublic
    )!;

    private static readonly MethodInfo _adjustBotHostilitySettings = typeof(LocationLifecycleService).GetMethod(
        "AdjustBotHostilitySettings",
        BindingFlags.Instance | BindingFlags.NonPublic
    )!;

    private readonly MongoId _sessionId = new();

    private RaidTimeAdjustmentService _raidTimeAdjustmentService = default!;
    private LocationLifecycleService _locationLifecycleService = default!;
    private LocationConfig _locationConfig = default!;
    private LocationTable _locationTable = default!;
    private ICloner _cloner = default!;
    private bool _originalForceLegacy;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _raidTimeAdjustmentService = di.GetService<RaidTimeAdjustmentService>();
        _locationLifecycleService = di.GetService<LocationLifecycleService>();
        _locationConfig = di.GetService<LocationConfig>();
        _locationTable = di.GetService<LocationTable>();
        _cloner = di.GetService<ICloner>();

        _originalForceLegacy = _locationConfig.ForceLegacyRaidAdjustments;
    }

    [TearDown]
    public void TearDown()
    {
        // The captured value, not false: a tree shipping the flag on would otherwise have it
        // silently flipped for every fixture that runs after this one
        _locationConfig.ForceLegacyRaidAdjustments = _originalForceLegacy;
    }

    /// <summary>
    /// The negative control: stock services, no force flag and no patches take the native path at
    /// every call site.
    /// </summary>
    [Test]
    public void NativePathIsTakenByDefault()
    {
        AssertRaidTimeCallSites(_raidTimeAdjustmentService, LootGenerationPath.Native, "stock");
        AssertLifecycleCallSites(_locationLifecycleService, LootGenerationPath.Native, "stock");
    }

    [Test]
    public void ForceLegacyFlagRoutesToTheLegacyPath()
    {
        _locationConfig.ForceLegacyRaidAdjustments = true;

        AssertRaidTimeCallSites(_raidTimeAdjustmentService, LootGenerationPath.Legacy, "force flag");
        AssertLifecycleCallSites(_locationLifecycleService, LootGenerationPath.Legacy, "force flag");
    }

    /// <summary>
    /// A mod compiled against the frozen contract can construct either service itself, and the frozen
    /// constructors have no native seam wired - such an instance has to run the C# body it was built
    /// for.
    /// </summary>
    [Test]
    public void TheFrozenConstructorsRouteToTheLegacyPath()
    {
        AssertRaidTimeCallSites(
            (RaidTimeAdjustmentService)Construct(typeof(RaidTimeAdjustmentService), narrowest: true),
            LootGenerationPath.Legacy,
            "frozen constructor"
        );

        AssertLifecycleCallSites(
            (LocationLifecycleService)Construct(typeof(LocationLifecycleService), narrowest: true),
            LootGenerationPath.Legacy,
            "frozen constructor"
        );
    }

    /// <summary>
    /// The negative control for the cases either side of it: hand-building a service off the
    /// container's own services is not by itself a reason to fall back.
    /// </summary>
    [Test]
    public void HandBuiltServicesWithStockServicesTakeTheNativePath()
    {
        AssertRaidTimeCallSites(
            (RaidTimeAdjustmentService)Construct(typeof(RaidTimeAdjustmentService)),
            LootGenerationPath.Native,
            "hand-built"
        );

        AssertLifecycleCallSites(
            (LocationLifecycleService)Construct(typeof(LocationLifecycleService)),
            LootGenerationPath.Native,
            "hand-built"
        );
    }

    /// <summary>
    /// A mod registering its own service with a higher TypePriority hands the container a subclass,
    /// whose overrides only the C# path can run. The check is per-service: the lifecycle service is
    /// not the one that was replaced, so it keeps its native path.
    /// </summary>
    [Test]
    public void AReplacedRaidTimeAdjustmentServiceRoutesToTheLegacyPathAtItsOwnCallSitesOnly()
    {
        var service = (RaidTimeAdjustmentService)Construct(typeof(TestRaidTimeAdjustmentServiceSubclass));

        AssertRaidTimeCallSites(service, LootGenerationPath.Legacy, "replaced raid time service");
        AssertLifecycleCallSites(_locationLifecycleService, LootGenerationPath.Native, "replaced raid time service");
    }

    /// <summary>
    /// The other half of the per-service scope: a replaced lifecycle service falls back at its own
    /// two passes and leaves the raid time service native.
    /// </summary>
    [Test]
    public void AReplacedLocationLifecycleServiceRoutesToTheLegacyPathAtItsOwnCallSitesOnly()
    {
        var service = (LocationLifecycleService)Construct(typeof(TestLocationLifecycleServiceSubclass));

        AssertLifecycleCallSites(service, LootGenerationPath.Legacy, "replaced lifecycle service");
        AssertRaidTimeCallSites(_raidTimeAdjustmentService, LootGenerationPath.Native, "replaced lifecycle service");
    }

    /// <summary>
    /// Both <see cref="RaidTimeAdjustmentService"/> call sites, each asserted against the path it was
    /// expected to take.
    /// </summary>
    private void AssertRaidTimeCallSites(RaidTimeAdjustmentService service, LootGenerationPath expected, string what)
    {
        service.GetRaidAdjustments(_sessionId, new GetRaidTimeRequest { Side = PmcSide, Location = RaidMap });
        Assert.That(service.LastPathTaken, Is.EqualTo(expected), $"{what}: GetRaidAdjustments took the wrong path");

        service.MakeAdjustmentsToMap(MapChanges(), Clone());
        Assert.That(service.LastPathTaken, Is.EqualTo(expected), $"{what}: MakeAdjustmentsToMap took the wrong path");
    }

    /// <summary>
    /// Both <see cref="LocationLifecycleService"/> call sites, each asserted against the path it was
    /// expected to take.
    /// </summary>
    private void AssertLifecycleCallSites(LocationLifecycleService service, LootGenerationPath expected, string what)
    {
        _adjustExtracts.Invoke(service, [ScavSide, RaidMap, Clone()]);
        Assert.That(service.LastPathTaken, Is.EqualTo(expected), $"{what}: AdjustExtracts took the wrong path");

        _adjustBotHostilitySettings.Invoke(service, [Clone()]);
        Assert.That(service.LastPathTaken, Is.EqualTo(expected), $"{what}: AdjustBotHostilitySettings took the wrong path");
    }

    private LocationBase Clone()
    {
        return _cloner.Clone(_locationTable.GetLocation(RaidMap)!.Base);
    }

    /// <summary>
    /// A parked <see cref="RaidChanges"/> as <c>GetRaidAdjustments</c> would have left it. Both loot
    /// percents are 100, so no case here scales the live multipliers.
    /// </summary>
    private static RaidChanges MapChanges()
    {
        return new RaidChanges
        {
            DynamicLootPercent = 100,
            StaticLootPercent = 100,
            SimulatedRaidStartSeconds = 600,
            RaidTimeMinutes = 30,
            NewSurviveTimeSeconds = 100,
            OriginalSurvivalTimeSeconds = 1000,
            ExitChanges = [],
        };
    }

    /// <summary>
    /// One service built by hand off the container's own services, on either the frozen constructor
    /// or the additive one the container picks.
    /// </summary>
    private static object Construct(Type type, bool narrowest = false)
    {
        // Both services carry the frozen constructor plus the additive overload the container uses;
        // take the widest unless the frozen one is what is under test
        var constructors = type.GetConstructors();
        var constructor = narrowest
            ? constructors.MinBy(candidate => candidate.GetParameters().Length)!
            : constructors.MaxBy(candidate => candidate.GetParameters().Length)!;

        var arguments = constructor.GetParameters().Select(parameter => DI.GetInstance().GetService(parameter.ParameterType)).ToArray();

        return constructor.Invoke(arguments);
    }

    /// <summary>
    /// Stands in for a mod-registered service: identical behaviour, different type. Chains the widest
    /// base constructor, so the native seam is wired and only the type check can fall back.
    /// </summary>
    private class TestRaidTimeAdjustmentServiceSubclass(
        ISptLogger<RaidTimeAdjustmentService> logger,
        GlobalTable globalTable,
        LocationTable locationTable,
        RandomUtil randomUtil,
        WeightedRandomHelper weightedRandomHelper,
        ProfileActivityService profileActivityService,
        LocationConfig locationConfig,
        RaidNativeRequestBuilder requestBuilder
    )
        : RaidTimeAdjustmentService(
            logger,
            globalTable,
            locationTable,
            randomUtil,
            weightedRandomHelper,
            profileActivityService,
            locationConfig,
            requestBuilder
        ) { }

    /// <summary>
    /// The same stand-in for the lifecycle service.
    /// </summary>
    private class TestLocationLifecycleServiceSubclass(
        ISptLogger<LocationLifecycleService> logger,
        GlobalTable globalTable,
        TemplateTable templateTable,
        LocationTable locationTable,
        RewardHelper rewardHelper,
        TimeUtil timeUtil,
        ProfileHelper profileHelper,
        BackupService backupService,
        ProfileActivityService profileActivityService,
        BotNameService botNameService,
        ICloner cloner,
        RaidTimeAdjustmentService raidTimeAdjustmentService,
        LocationLootGenerator locationLootGenerator,
        ServerLocalisationService serverLocalisationService,
        BotLootCacheService botLootCacheService,
        LootGenerator lootGenerator,
        MailSendService mailSendService,
        TraderHelper traderHelper,
        RandomUtil randomUtil,
        InRaidHelper inRaidHelper,
        PlayerScavGenerator playerScavGenerator,
        SaveServer saveServer,
        HealthHelper healthHelper,
        PmcChatResponseService pmcChatResponseService,
        PmcWaveGenerator pmcWaveGenerator,
        QuestHelper questHelper,
        InsuranceService insuranceService,
        MatchBotDetailsCacheService matchBotDetailsCacheService,
        BtrDeliveryService btrDeliveryService,
        LocationConfig locationConfig,
        InRaidConfig inRaidConfig,
        TraderConfig traderConfig,
        RagfairConfig ragfairConfig,
        HideoutConfig hideoutConfig,
        PmcConfig pmcConfig,
        LostOnDeathConfig lostOnDeathConfig,
        SeasonalEventConfig seasonalEventConfig,
        RaidNativeRequestBuilder requestBuilder
    )
        : LocationLifecycleService(
            logger,
            globalTable,
            templateTable,
            locationTable,
            rewardHelper,
            timeUtil,
            profileHelper,
            backupService,
            profileActivityService,
            botNameService,
            cloner,
            raidTimeAdjustmentService,
            locationLootGenerator,
            serverLocalisationService,
            botLootCacheService,
            lootGenerator,
            mailSendService,
            traderHelper,
            randomUtil,
            inRaidHelper,
            playerScavGenerator,
            saveServer,
            healthHelper,
            pmcChatResponseService,
            pmcWaveGenerator,
            questHelper,
            insuranceService,
            matchBotDetailsCacheService,
            btrDeliveryService,
            locationConfig,
            inRaidConfig,
            traderConfig,
            ragfairConfig,
            hideoutConfig,
            pmcConfig,
            lostOnDeathConfig,
            seasonalEventConfig,
            requestBuilder
        ) { }
}
