using System.Reflection;
using HarmonyLib;
using NUnit.Framework;
using SPTarkov.Common.Models.Logging;
using SPTarkov.Server.Core.Generators.Bot;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Generators.Weapons;
using SPTarkov.Server.Core.Helpers;
using SPTarkov.Server.Core.Helpers.Bot;
using SPTarkov.Server.Core.Helpers.InRaid;
using SPTarkov.Server.Core.Helpers.Items;
using SPTarkov.Server.Core.Helpers.Profile;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Mod;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native.Db;
using SPTarkov.Server.Core.Native.PlayerScav;
using SPTarkov.Server.Core.Servers;
using SPTarkov.Server.Core.Services;
using SPTarkov.Server.Core.Services.Bot;
using SPTarkov.Server.Core.Services.Commerce;
using SPTarkov.Server.Core.Services.Locales;
using SPTarkov.Server.Core.Services.Profile;
using SPTarkov.Server.Core.Services.Server;
using SPTarkov.Server.Core.Utils;
using SPTarkov.Server.Core.Utils.Cloners;
using Characters = SPTarkov.Server.Core.Models.Eft.Profile.Characters;
using ProfileInfo = SPTarkov.Server.Core.Models.Eft.Profile.Info;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Pins the dual-path dispatch for player scav generation: native by default, the retained 4.1.2 C#
/// implementation when <c>PlayerScavConfig.ForceLegacyPlayerScavGeneration</c> is set, when the
/// frozen 4.1.2 constructor built the instance (no native seam to dispatch to), when a mod
/// substituted this generator or the <see cref="BotInventoryGenerator"/> it rides, or when the bot
/// family itself declined its native path. Harmony patches on the frozen members are
/// <see cref="PlayerScavHookLivenessTests"/>' business; the one patch case here is the dispatcher
/// rule, which belongs with the routing decisions.
///
/// Mutates the shared <see cref="PlayerScavConfig"/> and <see cref="BotConfig"/> singletons and
/// patches process-wide, so both are restored per case and the fixture never runs in parallel.
/// </summary>
[TestFixture]
[NonParallelizable]
public class PlayerScavPathDispatchTests
{
    private const ulong NativeSeed = 424242;

    private static bool _prefixFired;
    private static bool _postfixFired;

    private PlayerScavGenerator _playerScavGenerator = default!;
    private PlayerScavConfig _playerScavConfig = default!;
    private BotConfig _botConfig = default!;

    private MongoId _sessionId;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _playerScavGenerator = di.GetService<PlayerScavGenerator>();
        _playerScavConfig = di.GetService<PlayerScavConfig>();
        _botConfig = di.GetService<BotConfig>();

        _sessionId = PlayerScavProfileFixture.Create();
        _playerScavGenerator.NativeTestSeed = NativeSeed;
    }

    [OneTimeTearDown]
    public void OneTimeTearDown()
    {
        _playerScavGenerator.NativeTestSeed = null;
    }

    /// <summary>
    /// The negative control: a stock container, no force flag and no patches take the native path.
    /// </summary>
    [Test]
    public void NativePathIsTakenByDefault()
    {
        var scav = Generate(_playerScavGenerator);

        Assert.That(_playerScavGenerator.LastPathTaken, Is.EqualTo(LootGenerationPath.Native));
        Assert.That(scav.Inventory!.Items, Is.Not.Empty, "the native path produced no inventory");
    }

    [Test]
    public void ForceLegacyFlagRoutesToTheLegacyPath()
    {
        _playerScavConfig.ForceLegacyPlayerScavGeneration = true;

        try
        {
            var scav = Generate(_playerScavGenerator);

            Assert.That(_playerScavGenerator.LastPathTaken, Is.EqualTo(LootGenerationPath.Legacy));
            Assert.That(scav.Inventory!.Items, Is.Not.Empty, "the legacy path produced no inventory");
        }
        finally
        {
            _playerScavConfig.ForceLegacyPlayerScavGeneration = false;
        }
    }

    /// <summary>
    /// A mod compiled against 4.1.2 can construct the generator itself, and the frozen constructor
    /// has no native seam wired - such an instance has to run the C# body it was built for.
    /// </summary>
    [Test]
    public void TheFrozenConstructorRoutesToTheLegacyPath()
    {
        var generator = Build(narrowest: true);

        Generate(generator);

        Assert.That(generator.LastPathTaken, Is.EqualTo(LootGenerationPath.Legacy));
    }

    /// <summary>
    /// The negative control for the substitution cases either side of it: hand-building the
    /// generator off the container's own services is not by itself a reason to fall back.
    /// </summary>
    [Test]
    public void AHandBuiltGeneratorWithStockServicesTakesTheNativePath()
    {
        var generator = Build();

        Generate(generator);

        Assert.That(generator.LastPathTaken, Is.EqualTo(LootGenerationPath.Native));
    }

    /// <summary>
    /// A mod registering its own generator with a higher TypePriority hands the container a
    /// subclass, whose overrides only the C# path can run. Built on the widest constructor, so the
    /// native seam is wired and the substituted type is the only reason left to fall back.
    /// </summary>
    [Test]
    public void AReplacedGeneratorRoutesToTheLegacyPath()
    {
        var generator = (PlayerScavGenerator)Construct(typeof(TestPlayerScavGeneratorSubclass));

        Generate(generator);

        Assert.That(generator.LastPathTaken, Is.EqualTo(LootGenerationPath.Legacy));
    }

    /// <summary>
    /// The dispatcher rule: <c>Generate</c> is the entry point, so a patch on it wraps whichever
    /// body runs rather than forcing a fall back, and the mod's hooks still see every call.
    /// </summary>
    [Test]
    public void AHarmonyPatchOnGenerateWrapsTheNativeBodyWithoutForcingLegacy()
    {
        var harmony = new Harmony("unit-tests.player-scav-path-dispatch.Generate");
        var target = typeof(PlayerScavGenerator).GetMethod(nameof(PlayerScavGenerator.Generate));
        Assert.That(target, Is.Not.Null, "Generate not found");

        _prefixFired = false;
        _postfixFired = false;
        try
        {
            harmony.Patch(
                target,
                prefix: new HarmonyMethod(typeof(PlayerScavPathDispatchTests), nameof(Prefix)),
                postfix: new HarmonyMethod(typeof(PlayerScavPathDispatchTests), nameof(Postfix))
            );

            Generate(_playerScavGenerator);

            Assert.That(
                _playerScavGenerator.LastPathTaken,
                Is.EqualTo(LootGenerationPath.Native),
                "a patch on the dispatcher forced legacy"
            );
            Assert.That(_prefixFired, Is.True, "prefix on Generate never ran");
            Assert.That(_postfixFired, Is.True, "postfix on Generate never ran");
        }
        finally
        {
            harmony.UnpatchSelf();
        }
    }

    /// <summary>
    /// The native player scav rides the bot family's export, so anything that de-natives bot
    /// inventory generation de-natives the player scav with it.
    /// </summary>
    [Test]
    public void ABotFamilyDeclineAlsoDeclinesThePlayerScav()
    {
        _botConfig.ForceLegacyBotGeneration = true;

        try
        {
            var scav = Generate(_playerScavGenerator);

            Assert.That(_playerScavGenerator.LastPathTaken, Is.EqualTo(LootGenerationPath.Legacy));
            Assert.That(scav.Inventory!.Items, Is.Not.Empty, "the legacy path produced no inventory");
        }
        finally
        {
            _botConfig.ForceLegacyBotGeneration = false;
        }
    }

    /// <summary>
    /// The native arm never calls <c>BotInventoryGenerator.GenerateInventory</c>, so a mod's
    /// subclass of it would be bypassed silently - and <c>BotInventoryGenerator.UseLegacyPath</c>
    /// carries no self-type check, so the fall back has to be decided on the player scav side.
    /// </summary>
    [Test]
    public void AReplacedBotInventoryGeneratorRoutesToTheLegacyPath()
    {
        var generator = (PlayerScavGenerator)Construct(typeof(PlayerScavGenerator), Construct(typeof(TestBotInventoryGeneratorSubclass)));

        Generate(generator);

        Assert.That(generator.LastPathTaken, Is.EqualTo(LootGenerationPath.Legacy));
    }

    /// <summary>
    /// Generation writes the new scav back into the profile, so every case starts from the same
    /// freshly seeded one.
    /// </summary>
    private PmcData Generate(PlayerScavGenerator generator)
    {
        PlayerScavProfileFixture.Reseed(_sessionId);

        return generator.Generate(_sessionId);
    }

    /// <summary>
    /// A generator built by hand off the container's own services, on either the frozen 4.1.2
    /// constructor or the additive one the container picks.
    /// </summary>
    private static PlayerScavGenerator Build(bool narrowest = false)
    {
        var constructors = typeof(PlayerScavGenerator).GetConstructors();
        var constructor = narrowest
            ? constructors.MinBy(candidate => candidate.GetParameters().Length)!
            : constructors.MaxBy(candidate => candidate.GetParameters().Length)!;

        return Seed((PlayerScavGenerator)constructor.Invoke(Arguments(constructor.GetParameters(), [])));
    }

    /// <summary>
    /// One instance off the widest constructor and the container's own services, with the given
    /// instances substituted for the parameters they fit - the shape DI would hand it if a mod had
    /// registered them.
    /// </summary>
    private static object Construct(Type type, params object[] substitutes)
    {
        var constructor = type.GetConstructors().MaxBy(candidate => candidate.GetParameters().Length)!;
        var instance = constructor.Invoke(Arguments(constructor.GetParameters(), substitutes));

        return instance is PlayerScavGenerator generator ? Seed(generator) : instance;
    }

    private static object?[] Arguments(ParameterInfo[] parameters, object[] substitutes)
    {
        return
        [
            .. parameters.Select(parameter =>
                substitutes.FirstOrDefault(substitute => parameter.ParameterType.IsInstanceOfType(substitute))
                ?? DI.GetInstance().GetService(parameter.ParameterType)
            ),
        ];
    }

    private static PlayerScavGenerator Seed(PlayerScavGenerator generator)
    {
        generator.NativeTestSeed = NativeSeed;

        return generator;
    }

    private static void Prefix()
    {
        _prefixFired = true;
    }

    private static void Postfix()
    {
        _postfixFired = true;
    }

    /// <summary>
    /// Stands in for a mod-registered generator: identical behaviour, different type. Chains the
    /// widest base constructor, so the native seam is wired and only the type check can fall back.
    /// </summary>
    private class TestPlayerScavGeneratorSubclass(
        ISptLogger<PlayerScavGenerator> logger,
        GlobalTable globalTable,
        RandomUtil randomUtil,
        ItemHelper itemHelper,
        BotGeneratorHelper botGeneratorHelper,
        SaveServer saveServer,
        ProfileHelper profileHelper,
        BotHelper botHelper,
        FenceService fenceService,
        BotLootCacheService botLootCacheService,
        ServerLocalisationService serverLocalisationService,
        BotInventoryContainerService botInventoryContainerService,
        BotGenerator botGenerator,
        PlayerScavConfig playerScavConfig,
        ICloner cloner,
        TimeUtil timeUtil,
        PlayerScavNativeRequestBuilder requestBuilder,
        BotInventoryGenerator botInventoryGenerator,
        SeasonalEventService seasonalEventService,
        IReadOnlyList<SptMod> loadedMods,
        DbPublisher dbPublisher
    )
        : PlayerScavGenerator(
            logger,
            globalTable,
            randomUtil,
            itemHelper,
            botGeneratorHelper,
            saveServer,
            profileHelper,
            botHelper,
            fenceService,
            botLootCacheService,
            serverLocalisationService,
            botInventoryContainerService,
            botGenerator,
            playerScavConfig,
            cloner,
            timeUtil,
            requestBuilder,
            botInventoryGenerator,
            seasonalEventService,
            loadedMods,
            dbPublisher
        ) { }

    /// <summary>
    /// Stands in for a mod-registered bot inventory generator. Chains the widest base constructor,
    /// so that generator's own dispatch stays native and the player scav side's type check is the
    /// only thing that can decline.
    /// </summary>
    private class TestBotInventoryGeneratorSubclass(
        ISptLogger<BotInventoryGenerator> logger,
        RandomUtil randomUtil,
        ProfileActivityService profileActivityService,
        BotWeaponGenerator botWeaponGenerator,
        BotLootGenerator botLootGenerator,
        BotGeneratorHelper botGeneratorHelper,
        ProfileHelper profileHelper,
        BotHelper botHelper,
        WeightedRandomHelper weightedRandomHelper,
        ItemHelper itemHelper,
        WeatherHelper weatherHelper,
        ServerLocalisationService serverLocalisationService,
        BotEquipmentFilterService botEquipmentFilterService,
        BotEquipmentModPoolService botEquipmentModPoolService,
        BotEquipmentModGenerator botEquipmentModGenerator,
        BotInventoryContainerService botInventoryContainerService,
        BotConfig botConfig,
        PmcConfig pmcConfig,
        IReadOnlyList<SptMod> loadedMods,
        DbPublisher dbPublisher
    )
        : BotInventoryGenerator(
            logger,
            randomUtil,
            profileActivityService,
            botWeaponGenerator,
            botLootGenerator,
            botGeneratorHelper,
            profileHelper,
            botHelper,
            weightedRandomHelper,
            itemHelper,
            weatherHelper,
            serverLocalisationService,
            botEquipmentFilterService,
            botEquipmentModPoolService,
            botEquipmentModGenerator,
            botInventoryContainerService,
            botConfig,
            pmcConfig,
            loadedMods,
            dbPublisher
        ) { }
}

/// <summary>
/// The profile <c>PlayerScavGenerator.Generate</c> needs in <see cref="SaveServer"/>, shared by the
/// three player scav fixtures. Deliberately minimal: only the members the generator and the shell
/// below it actually read - the Fence entry the karma level is derived from, the Info fields copied
/// onto the scav, the ids the metadata block reads back and the (empty) bonus list the cooldown
/// timer sums over. Health is omitted: the limb copy is null-guarded.
/// </summary>
internal static class PlayerScavProfileFixture
{
    /// <summary>
    /// Fence standing 0, so every fixture selects karma level "0" - the shipped level whose
    /// <c>lootItemsToAddChancePercent</c> is empty.
    /// </summary>
    private const double FenceStanding = 0;

    /// <summary>
    /// The scav's own id, which <c>Generate</c> takes from <c>PmcData.Savage</c> when no previous
    /// scav exists. Fixed, so two arms of a parity comparison start from the same one.
    /// </summary>
    private static readonly MongoId _savageId = new("65f1a0d4b8e2c7f39a4b6d81");

    internal static MongoId Create()
    {
        var sessionId = new MongoId();
        DI.GetInstance().GetService<SaveServer>().CreateProfile(new ProfileInfo { ProfileId = sessionId });
        Reseed(sessionId);

        return sessionId;
    }

    /// <summary>
    /// Restores the profile to its starting state. <c>Generate</c> writes the scav it built back
    /// into the profile, where the next call reads it as the previous scav, so anything comparing
    /// or repeating generations has to re-seed between them.
    /// </summary>
    internal static void Reseed(MongoId sessionId)
    {
        DI.GetInstance().GetService<SaveServer>().GetProfile(sessionId).CharacterData = new Characters
        {
            PmcData = new PmcData
            {
                Id = new MongoId("65f1a0d4b8e2c7f39a4b6d80"),
                Aid = 1,
                SessionId = sessionId,
                Savage = _savageId,
                Info = new Info
                {
                    Nickname = "PlayerScavFixture",
                    RegistrationDate = 0,
                    GameVersion = "standard",
                },
                Bonuses = [],
                Encyclopedia = [],
                TradersInfo = new Dictionary<MongoId, TraderInfo>
                {
                    {
                        Traders.FENCE,
                        new TraderInfo { Standing = FenceStanding }
                    },
                },
            },
            ScavData = new PmcData(),
        };
    }
}
