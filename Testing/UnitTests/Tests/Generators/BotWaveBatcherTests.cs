using NUnit.Framework;
using SPTarkov.Common.Models.Logging;
using SPTarkov.Server.Core.Generators.Bot;
using SPTarkov.Server.Core.Helpers;
using SPTarkov.Server.Core.Helpers.Bot;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Spt.Bots;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Servers;
using SPTarkov.Server.Core.Services.Bot;
using SPTarkov.Server.Core.Services.Items;
using SPTarkov.Server.Core.Services.Server;
using SPTarkov.Server.Core.Utils;
using SPTarkov.Server.Core.Utils.Cloners;
using ProfileInfo = SPTarkov.Server.Core.Models.Eft.Profile.Info;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Pins the batched wave path: a default wave comes back complete, and each reason the batcher
/// declines hands the caller null so it falls through to the per-bot path. Mutates the shared config
/// singleton, so it restores it and never runs in parallel with other fixtures.
/// </summary>
[TestFixture]
[NonParallelizable]
public class BotWaveBatcherTests
{
    private BotWaveBatcher _batcher = default!;
    private BotConfig _botConfig = default!;
    private MongoId _sessionId;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();
        _batcher = di.GetService<BotWaveBatcher>();
        _botConfig = di.GetService<BotConfig>();
        _sessionId = new MongoId();
        di.GetService<SaveServer>().CreateProfile(new ProfileInfo { ProfileId = _sessionId });
    }

    private static BotGenerationDetails BuildWaveDetails(int count = 3)
    {
        return new BotGenerationDetails
        {
            Role = "assault",
            Side = "Savage",
            BotDifficulty = "normal",
            GameVersion = "standard",
            BotCountToGenerate = count,
        };
    }

    [Test]
    public void ABatchedWaveProducesCompleteBots()
    {
        var wave = _batcher.TryGenerateWave(_sessionId, BuildWaveDetails());

        Assert.That(wave, Is.Not.Null, "default configuration should take the batch path");
        Assert.That(wave!, Has.Count.EqualTo(3));
        foreach (var bot in wave!)
        {
            Assert.That(bot!.Inventory?.Items, Is.Not.Empty, "bot came back without an inventory");
            Assert.That(bot.Id, Is.Not.EqualTo(default(MongoId)));
        }

        // GenerateInventoryId reroots every bot onto a fresh equipment id - all distinct
        Assert.That(wave!.Select(bot => bot!.Inventory!.Equipment).Distinct().Count(), Is.EqualTo(3));
    }

    [Test]
    public void ForcePerBotGenerationDeclinesTheBatch()
    {
        _botConfig.ForcePerBotGeneration = true;
        try
        {
            Assert.That(_batcher.TryGenerateWave(_sessionId, BuildWaveDetails()), Is.Null);
        }
        finally
        {
            _botConfig.ForcePerBotGeneration = false;
        }
    }

    [Test]
    public void ForceLegacyBotGenerationDeclinesTheBatch()
    {
        _botConfig.ForceLegacyBotGeneration = true;
        try
        {
            Assert.That(_batcher.TryGenerateWave(_sessionId, BuildWaveDetails()), Is.Null);
        }
        finally
        {
            _botConfig.ForceLegacyBotGeneration = false;
        }
    }

    /// <summary>
    /// A mod registering its own BotGenerator with a higher TypePriority hands the container a
    /// subclass, which only the per-bot path routes through.
    /// </summary>
    [Test]
    public void ASubstitutedBotGeneratorDeclinesTheBatch()
    {
        var batcher = (BotWaveBatcher)Construct(typeof(BotWaveBatcher), Construct(typeof(TestBotGeneratorSubclass)));

        Assert.That(batcher.TryGenerateWave(_sessionId, BuildWaveDetails()), Is.Null);
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
    private class TestBotGeneratorSubclass(
        ISptLogger<BotGenerator> logger,
        TemplateTable templateTable,
        GlobalTable globalTable,
        BotTable botTable,
        RandomUtil randomUtil,
        BotInventoryGenerator botInventoryGenerator,
        BotLevelGenerator botLevelGenerator,
        BotEquipmentFilterService botEquipmentFilterService,
        WeightedRandomHelper weightedRandomHelper,
        BotHelper botHelper,
        SeasonalEventService seasonalEventService,
        ItemFilterService itemFilterService,
        BotNameService botNameService,
        BotConfig botConfig,
        PmcConfig pmcConfig,
        ICloner cloner
    )
        : BotGenerator(
            logger,
            templateTable,
            globalTable,
            botTable,
            randomUtil,
            botInventoryGenerator,
            botLevelGenerator,
            botEquipmentFilterService,
            weightedRandomHelper,
            botHelper,
            seasonalEventService,
            itemFilterService,
            botNameService,
            botConfig,
            pmcConfig,
            cloner
        ) { }
}
