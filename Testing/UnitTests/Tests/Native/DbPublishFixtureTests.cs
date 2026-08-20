using NUnit.Framework;
using SPTarkov.Server.Core.Loaders;
using SPTarkov.Server.Core.Models.Eft.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Eft.Hideout;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native.Db;
using SPTarkov.Server.Core.Utils;
using SPTarkov.Server.Core.Utils.Json;

namespace UnitTests.Tests.Native;

[TestFixture]
[Explicit("Phase 4: writes the configs-root publish envelope for the Rust phase4_configs_root fidelity test")]
public class DbPublishFixtureTests
{
    [Test]
    public async Task WriteConfigsRootFixture()
    {
        // The crossing under test: ConfigLoader reads SPT_Data/configs with its own bespoke
        // options, the envelope writes with the shared ones. Loading straight off disk instead of
        // out of the container keeps this a test of that crossing only - no IOnLoad has mutated a
        // config on the way through.
        var configs = await ConfigLoader.Initialize();

        // Publishes the static write options BuildPublishEnvelope reads, without standing up a
        // container (the LocationLootGeneratorNativeTests precedent). SptJsonConverterRegistrator
        // is the solution's only IJsonConverterRegistrator, so these are the options DI builds.
        _ = new JsonUtil([new SptJsonConverterRegistrator()]);

        // Every table stubbed empty: the configs root is what this pair gates, and a full-database
        // envelope would bury it under a hundred megabytes the Rust side has to reparse.
        var location = new Location();
        var envelope = DbPayloadProjection.BuildPublishEnvelope(
            new TemplateTable
            {
                Character = [],
                CustomisationStorage = [],
                Items = [],
                Prestige = new Prestige { Elements = [] },
                Quests = [],
                RepeatableQuests = new RepeatableQuestDatabase(),
                Handbook = new HandbookBase { Categories = [], Items = [] },
                Customization = [],
                Dialogue = new TraderDialogs { Elements = [] },
                Profiles = [],
                Prices = [],
                DefaultEquipmentPresets = [],
                Achievements = [],
                CustomAchievements = [],
                LocationServices = new LocationServices(),
            },
            new TradersTable(),
            new GlobalTable
            {
                Configuration = new GlobalConfig
                {
                    Mastering = [],
                    ArenaEftTransferSettings = new ArenaEftTransferSettings(),
                    RestrictionsInRaid = [],
                    EventType = [],
                },
                LocationInfection = [],
                BotPresets = [],
                BotWeaponScatterings = [],
                ItemPresets = [],
            },
            new LocationTable
            {
                Bigmap = location,
                Factory4Day = location,
                Factory4Night = location,
                Interchange = location,
                Laboratory = location,
                Lighthouse = location,
                RezervBase = location,
                Shoreline = location,
                TarkovStreets = location,
                Labyrinth = location,
                Woods = location,
                Sandbox = location,
                SandboxHigh = location,
                Base = new LocationsBase(),
            },
            new HideoutTable
            {
                Areas = [],
                CustomAreas = null,
                Customisation = new HideoutCustomisation(),
                Production = new HideoutProductionData(),
                Settings = new HideoutSettingsBase(),
                Qte = [],
            },
            configs
        );

        // System.IO.Path is qualified: Models.Eft.Common.Tables declares a Path of its own.
        var path = System.IO.Path.Combine(System.IO.Path.GetTempPath(), "spt-phase4-configs.json");
        File.WriteAllBytes(path, envelope);

        TestContext.Out.WriteLine($"configs envelope written to {path}");

        // One loaded config per ConfigTypes entry, or the Rust half's literal 28-kind list would
        // fail for a reason that has nothing to do with serializer fidelity.
        Assert.That(configs.Count, Is.EqualTo(Enum.GetValues<ConfigTypes>().Length), "every ConfigTypes entry must have loaded a config");
    }
}
