using NUnit.Framework;
using SPTarkov.Common.Models.Logging;
using SPTarkov.Server.Core.Generators.Bot;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Generators.Weapons;
using SPTarkov.Server.Core.Generators.Weapons.Implementations;
using SPTarkov.Server.Core.Helpers;
using SPTarkov.Server.Core.Helpers.Bot;
using SPTarkov.Server.Core.Helpers.Items;
using SPTarkov.Server.Core.Helpers.Profile;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Bots;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Servers;
using SPTarkov.Server.Core.Services;
using SPTarkov.Server.Core.Services.Bot;
using SPTarkov.Server.Core.Services.Locales;
using SPTarkov.Server.Core.Utils;
using SPTarkov.Server.Core.Utils.Cloners;
using ProfileInfo = SPTarkov.Server.Core.Models.Eft.Profile.Info;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Pins the four reasons bot generation falls back to the retained 4.1.2 C# implementation - the
/// config flag, a replaced sibling generator and a modified mag-gen component set (a Harmony patch
/// is the fourth, covered by BotHookLivenessTests) - plus the container grid state the native path
/// has to hand back when the caller keeps the cache. Mutates the shared config singleton, so it
/// restores it and never runs in parallel with other fixtures.
/// </summary>
[TestFixture]
[NonParallelizable]
public class BotPathDispatchTests
{
    private BotInventoryGenerator _botInventoryGenerator = default!;
    private BotEquipmentFilterService _botEquipmentFilterService = default!;
    private BotInventoryContainerService _botInventoryContainerService = default!;
    private BotConfig _botConfig = default!;
    private BotTable _botTable = default!;
    private ICloner _cloner = default!;

    private MongoId _sessionId;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _botInventoryGenerator = di.GetService<BotInventoryGenerator>();
        _botEquipmentFilterService = di.GetService<BotEquipmentFilterService>();
        _botInventoryContainerService = di.GetService<BotInventoryContainerService>();
        _botConfig = di.GetService<BotConfig>();
        _botTable = di.GetService<BotTable>();
        _cloner = di.GetService<ICloner>();

        _sessionId = new MongoId();
        di.GetService<SaveServer>().CreateProfile(new ProfileInfo { ProfileId = _sessionId });
    }

    [Test]
    public void NativePathIsTakenByDefault()
    {
        var inventory = Generate(_botInventoryGenerator, BuildDetails());

        Assert.That(_botInventoryGenerator.LastPathTaken, Is.EqualTo(LootGenerationPath.Native));
        Assert.That(inventory.Items, Is.Not.Empty);
    }

    [Test]
    public void ForceLegacyFlagRoutesToTheLegacyPath()
    {
        _botConfig.ForceLegacyBotGeneration = true;
        try
        {
            var inventory = Generate(_botInventoryGenerator, BuildDetails());

            Assert.That(_botInventoryGenerator.LastPathTaken, Is.EqualTo(LootGenerationPath.Legacy));
            Assert.That(inventory.Items, Is.Not.Empty);
        }
        finally
        {
            _botConfig.ForceLegacyBotGeneration = false;
        }
    }

    /// <summary>
    /// The negative control for the two substitution tests below: hand-building the generator off
    /// the container's own services is not by itself a reason to fall back.
    /// </summary>
    [Test]
    public void AHandBuiltGeneratorWithStockServicesTakesTheNativePath()
    {
        var generator = BuildGenerator();

        Generate(generator, BuildDetails());

        Assert.That(generator.LastPathTaken, Is.EqualTo(LootGenerationPath.Native));
    }

    /// <summary>
    /// A mod registering its own BotLootGenerator with a higher TypePriority hands the container a
    /// subclass, whose overrides only the C# path can run.
    /// </summary>
    [Test]
    public void AReplacedSiblingGeneratorRoutesToTheLegacyPath()
    {
        var generator = BuildGenerator(Construct(typeof(TestBotLootGeneratorSubclass)));

        var inventory = Generate(generator, BuildDetails());

        Assert.That(generator.LastPathTaken, Is.EqualTo(LootGenerationPath.Legacy));
        Assert.That(inventory.Items, Is.Not.Empty);
    }

    /// <summary>
    /// A mod contributing a fifth IInventoryMagGen means magazine generation no longer matches the
    /// four components the native path folded in.
    /// </summary>
    [Test]
    public void AnExtraMagGenComponentRoutesToTheLegacyPath()
    {
        var magGens = new List<IInventoryMagGen>
        {
            (BarrelInventoryMagGen)DI.GetInstance().GetService(typeof(BarrelInventoryMagGen)),
            (ExternalInventoryMagGen)DI.GetInstance().GetService(typeof(ExternalInventoryMagGen)),
            (InternalMagazineInventoryMagGen)DI.GetInstance().GetService(typeof(InternalMagazineInventoryMagGen)),
            (UbglExternalMagGen)DI.GetInstance().GetService(typeof(UbglExternalMagGen)),
            new StubInventoryMagGen(),
        };

        var generator = BuildGenerator(Construct(typeof(BotWeaponGenerator), magGens));

        var inventory = Generate(generator, BuildDetails());

        Assert.That(generator.LastPathTaken, Is.EqualTo(LootGenerationPath.Legacy));
        Assert.That(inventory.Items, Is.Not.Empty);
    }

    /// <summary>
    /// The player scav shape: the caller keeps the container cache because it adds more loot into
    /// those same containers afterwards, so the native path has to restore grid state that lines up
    /// with the inventory it returned.
    /// </summary>
    [Test]
    public void NativePathRestoresContainerGridsWhenTheCacheIsKept()
    {
        var botId = new MongoId();
        var details = BuildDetails();
        details.IsPlayerScav = true;
        details.ClearBotContainerCacheAfterGeneration = false;

        try
        {
            var inventory = Generate(_botInventoryGenerator, details, botId);

            Assert.That(_botInventoryGenerator.LastPathTaken, Is.EqualTo(LootGenerationPath.Native));

            var containers = _botInventoryContainerService.GetBotContainer(botId);
            Assert.That(containers, Is.Not.Null.And.Not.Empty, "native path left no container state behind");

            var anyGridFilled = false;
            foreach (var (slot, container) in containers!)
            {
                var inventoryItem = inventory.Items!.FirstOrDefault(item => item.Id == container.ContainerInventoryItem.Id);
                Assert.That(inventoryItem, Is.Not.Null, $"cached container for {slot} is not in the returned inventory");
                Assert.That(inventoryItem!.SlotId, Is.EqualTo(slot.ToString()));
                Assert.That(inventoryItem.Template, Is.EqualTo(container.ContainerDbItem.Id));

                var dbGrids = container.ContainerDbItem.Properties!.Grids!.ToList();
                Assert.That(container.ContainerGridDetails.Count, Is.EqualTo(dbGrids.Count), $"grid count mismatch for {slot}");

                for (var i = 0; i < dbGrids.Count; i++)
                {
                    var gridMap = container.ContainerGridDetails[i].GridMap;
                    Assert.That(gridMap.GetLength(0), Is.EqualTo(dbGrids[i].Properties!.CellsV), $"{slot} grid {i} row count");
                    Assert.That(gridMap.GetLength(1), Is.EqualTo(dbGrids[i].Properties!.CellsH), $"{slot} grid {i} column count");

                    anyGridFilled |= gridMap.Cast<int>().Any(cell => cell != 0);
                }
            }

            // Blank grids would satisfy every check above while telling later callers the bot is
            // carrying nothing
            Assert.That(anyGridFilled, Is.True, "every restored grid is empty - no occupancy was carried back");
        }
        finally
        {
            // PlayerScavGenerator:96 clears it once it is done adding loot
            _botInventoryContainerService.ClearCache(botId);
        }
    }

    private BotBaseInventory Generate(BotInventoryGenerator generator, BotGenerationDetails details, MongoId? botId = null)
    {
        var template = _cloner.Clone(_botTable.Types["assault"])!;
        if (!details.IsPlayerScav)
        {
            _botEquipmentFilterService.FilterBotEquipment(_sessionId, template, details);
        }

        return generator.GenerateInventory(botId ?? new MongoId(), _sessionId, template, details);
    }

    private static BotGenerationDetails BuildDetails()
    {
        return new BotGenerationDetails
        {
            Role = "assault",
            RoleLowercase = "assault",
            Side = "Savage",
            BotDifficulty = "normal",
            GameVersion = "standard",
            BotLevel = 1,
        };
    }

    /// <summary>
    /// A BotInventoryGenerator built by hand off the container's own services, with the given
    /// instances substituted for the parameters they fit - the shape DI would hand it if a mod had
    /// registered them.
    /// </summary>
    private static BotInventoryGenerator BuildGenerator(params object[] substitutes)
    {
        return (BotInventoryGenerator)Construct(typeof(BotInventoryGenerator), substitutes);
    }

    private static object Construct(Type type, params object[] substitutes)
    {
        var constructor = type.GetConstructors().Single();
        var arguments = constructor
            .GetParameters()
            .Select(parameter =>
                substitutes.FirstOrDefault(substitute => parameter.ParameterType.IsInstanceOfType(substitute))
                ?? DI.GetInstance().GetService(parameter.ParameterType)
            )
            .ToArray();

        return constructor.Invoke(arguments);
    }

    /// <summary>
    /// Stands in for a mod-registered generator: identical behaviour, different type.
    /// </summary>
    private class TestBotLootGeneratorSubclass(
        ISptLogger<BotLootGenerator> logger,
        RandomUtil randomUtil,
        ItemHelper itemHelper,
        InventoryHelper inventoryHelper,
        HandbookHelper handbookHelper,
        BotGeneratorHelper botGeneratorHelper,
        BotWeaponGenerator botWeaponGenerator,
        WeightedRandomHelper weightedRandomHelper,
        BotHelper botHelper,
        BotLootCacheService botLootCacheService,
        ServerLocalisationService serverLocalisationService,
        BotConfig botConfig,
        PmcConfig pmcConfig,
        ICloner cloner
    )
        : BotLootGenerator(
            logger,
            randomUtil,
            itemHelper,
            inventoryHelper,
            handbookHelper,
            botGeneratorHelper,
            botWeaponGenerator,
            weightedRandomHelper,
            botHelper,
            botLootCacheService,
            serverLocalisationService,
            botConfig,
            pmcConfig,
            cloner
        ) { }

    /// <summary>
    /// A fifth mag-gen component that never claims a request, so it changes the component set
    /// without changing what the legacy path generates.
    /// </summary>
    private class StubInventoryMagGen : IInventoryMagGen
    {
        public int GetPriority()
        {
            return int.MaxValue;
        }

        public bool CanHandleInventoryMagGen(InventoryMagGen inventoryMagGen)
        {
            return false;
        }

        public void Process(InventoryMagGen inventoryMagGen)
        {
            throw new InvalidOperationException("stub mag gen should never be selected");
        }
    }
}
