using System.Text;
using System.Text.Json;
using NUnit.Framework;
using SPTarkov.Server.Core.Generators.Bot;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Generators.Weapons;
using SPTarkov.Server.Core.Helpers;
using SPTarkov.Server.Core.Helpers.Bot;
using SPTarkov.Server.Core.Helpers.InRaid;
using SPTarkov.Server.Core.Helpers.Items;
using SPTarkov.Server.Core.Helpers.Profile;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Match;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Bots;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Mod;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native;
using SPTarkov.Server.Core.Native.Bot;
using SPTarkov.Server.Core.Native.Db;
using SPTarkov.Server.Core.Servers;
using SPTarkov.Server.Core.Services.Bot;
using SPTarkov.Server.Core.Services.Profile;
using SPTarkov.Server.Core.Services.Server;
using SPTarkov.Server.Core.Utils;
using SPTarkov.Server.Core.Utils.Cloners;
using ProfileInfo = SPTarkov.Server.Core.Models.Eft.Profile.Info;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Pins the resident-DB epoch protocol on the bot generation native paths: an eligible wave names
/// an epoch and never sends the views override, the kill switch and untrusted mods fall back to
/// the override, a generator built on the frozen 4.1.2 constructor always overrides, a native-side
/// epoch desync self-heals through one republish plus retry, and — the flip's core promise — a
/// resident send and an override send generate identical bots field for field. Epochs are
/// process-global (other fixtures publish too), so every assertion is relative. Mutates the shared
/// config singleton, so it restores it and never runs in parallel with other fixtures.
/// </summary>
[TestFixture]
[NonParallelizable]
public class BotResidentDbTests
{
    private const ulong Seed = 424242;

    private BotWaveBatcher _batcher = default!;
    private BotInventoryGenerator _botInventoryGenerator = default!;
    private BotWeaponGenerator _botWeaponGenerator = default!;
    private BotLootGenerator _botLootGenerator = default!;
    private BotEquipmentModGenerator _botEquipmentModGenerator = default!;
    private ProfileHelper _profileHelper = default!;
    private ItemHelper _itemHelper = default!;
    private WeatherHelper _weatherHelper = default!;
    private ProfileActivityService _profileActivityService = default!;
    private BotEquipmentFilterService _botEquipmentFilterService = default!;
    private BotHelper _botHelper = default!;
    private BotConfig _botConfig = default!;
    private PmcConfig _pmcConfig = default!;
    private BotTable _botTable = default!;
    private ICloner _cloner = default!;
    private DatabaseMutationStamp _stamp = default!;
    private DbPublisher _publisher = default!;

    private MongoId _sessionId;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _batcher = di.GetService<BotWaveBatcher>();
        _botInventoryGenerator = di.GetService<BotInventoryGenerator>();
        _botWeaponGenerator = di.GetService<BotWeaponGenerator>();
        _botLootGenerator = di.GetService<BotLootGenerator>();
        _botEquipmentModGenerator = di.GetService<BotEquipmentModGenerator>();
        _profileHelper = di.GetService<ProfileHelper>();
        _itemHelper = di.GetService<ItemHelper>();
        _weatherHelper = di.GetService<WeatherHelper>();
        _profileActivityService = di.GetService<ProfileActivityService>();
        _botEquipmentFilterService = di.GetService<BotEquipmentFilterService>();
        _botHelper = di.GetService<BotHelper>();
        _botConfig = di.GetService<BotConfig>();
        _pmcConfig = di.GetService<PmcConfig>();
        _botTable = di.GetService<BotTable>();
        _cloner = di.GetService<ICloner>();
        _stamp = di.GetService<DatabaseMutationStamp>();
        _publisher = di.GetService<DbPublisher>();

        _sessionId = new MongoId();
        di.GetService<SaveServer>().CreateProfile(new ProfileInfo { ProfileId = _sessionId });
    }

    [OneTimeTearDown]
    public void OneTimeTearDown()
    {
        _botConfig.DisableNativeRequestCache = false;
        _botConfig.TrustNativeRequestCacheWithMods = true;
        // leave the shared container fresher than we found it for whatever fixture runs next
        _stamp.Bump();
    }

    /// <summary>
    /// One wave through the batch path. Fails fast on a silent decline to the per-bot path or an
    /// incomplete wave before asserting anything about the send.
    /// </summary>
    private void GenerateWave(BotWaveBatcher batcher)
    {
        var wave = batcher.TryGenerateWave(
            _sessionId,
            new BotGenerationDetails
            {
                Role = "assault",
                RoleLowercase = "assault",
                Side = "Savage",
                BotDifficulty = "normal",
                GameVersion = "standard",
                BotCountToGenerate = 3,
            }
        );

        Assert.That(wave, Is.Not.Null, "the wave declined the batch path");
        Assert.That(wave!, Has.Count.EqualTo(3), "the batch generated an incomplete wave");
    }

    /// <summary>
    /// The batch path's twin for the single-bot dispatch site. Same fail-fast contract as
    /// <see cref="GenerateWave" />: a decline to the expected path or an empty inventory fails here,
    /// before the caller asserts anything about the send. Defaults to the assault-at-level-1 case on
    /// the native path; the nighttime case passes its own details, template and expected path.
    /// </summary>
    private void GenerateSingleBot(
        BotInventoryGenerator generator,
        BotGenerationDetails? details = null,
        string templateKey = "assault",
        LootGenerationPath expected = LootGenerationPath.Native
    )
    {
        details ??= new BotGenerationDetails
        {
            Role = "assault",
            RoleLowercase = "assault",
            Side = "Savage",
            BotDifficulty = "normal",
            GameVersion = "standard",
            BotLevel = 1,
        };
        var template = _cloner.Clone(_botTable.Types[templateKey])!;
        _botEquipmentFilterService.FilterBotEquipment(_sessionId, template, details);

        var inventory = generator.GenerateInventory(new MongoId(), _sessionId, template, details);

        Assert.That(generator.LastPathTaken, Is.EqualTo(expected), $"generation did not take the {expected} path");
        Assert.That(inventory.Items, Is.Not.Empty, $"the {expected} path generated no inventory");
    }

    [Test]
    public void EligibleGenerationBuildsOffTheResidentDb()
    {
        GenerateWave(_batcher);

        Assert.That(_batcher.LastSendIncludedViewsOverride, Is.False, "an eligible wave must not send the override");
    }

    /// <summary>
    /// The single-bot dispatch site's eligible arm, on the container-resolved generator — the path
    /// every wave that declines batching lands on. Pins that DI selects the epoch-protocol
    /// constructor overload (the frozen one has no publisher, so it could only override) and that a
    /// generation stamped with the resident epoch comes back whole.
    /// </summary>
    [Test]
    public void EligibleSingleBotGenerationBuildsOffTheResidentDb()
    {
        GenerateSingleBot(_botInventoryGenerator);

        Assert.That(
            _botInventoryGenerator.LastSendIncludedViewsOverride,
            Is.False,
            "an eligible single-bot generation must not send the override"
        );
    }

    [Test]
    public void KillSwitchForcesTheViewsOverride()
    {
        _botConfig.DisableNativeRequestCache = true;
        try
        {
            GenerateWave(_batcher);

            Assert.That(_batcher.LastSendIncludedViewsOverride, Is.True, "the kill switch must force the override");
        }
        finally
        {
            _botConfig.DisableNativeRequestCache = false;
        }
    }

    [Test]
    public void ModsLoadedWithoutTheTrustFlagForceTheViewsOverride()
    {
        // The gate only reads Count, so a placeholder element stands in for a real mod
        var modded = BuildBatcherWithMods(DI.GetInstance(), new SptMod[] { null! });

        _botConfig.TrustNativeRequestCacheWithMods = false;
        try
        {
            GenerateWave(modded);

            Assert.That(modded.LastSendIncludedViewsOverride, Is.True, "a loaded mod without the trust flag disables residency");
        }
        finally
        {
            _botConfig.TrustNativeRequestCacheWithMods = true;
        }
    }

    [Test]
    public void TheTrustFlagKeepsTheResidentPathLiveWithModsLoaded()
    {
        if (!WriteBarrier.Installed)
        {
            Assert.Ignore("write barriers are Ceciler-injected in Release builds only");
        }

        var modded = BuildBatcherWithMods(DI.GetInstance(), new SptMod[] { null! });

        _botConfig.TrustNativeRequestCacheWithMods = true;
        try
        {
            GenerateWave(modded);

            Assert.That(
                modded.LastSendIncludedViewsOverride,
                Is.False,
                "the trust flag should keep the resident path live despite the mod"
            );
        }
        finally
        {
            _botConfig.TrustNativeRequestCacheWithMods = true;
        }
    }

    /// <summary>
    /// The single-bot dispatch site's half of the protocol: a generator built on the frozen 4.1.2
    /// constructor has no publisher and no mod list, so every native send carries the override.
    /// </summary>
    [Test]
    public void AGeneratorBuiltOnTheFrozenConstructorAlwaysSendsTheOverride()
    {
        var frozen = BuildGeneratorWithFrozenConstructor(DI.GetInstance());

        GenerateSingleBot(frozen);

        Assert.That(frozen.LastSendIncludedViewsOverride, Is.True, "no publisher means no residency eligibility");
    }

    [Test]
    public void ANativeSideEpochDesyncSelfHealsThroughOneRetry()
    {
        // Settle the publisher's remembered epoch first, so the desync below is the only miss
        _publisher.EnsureCurrent();

        // Desync: a direct native publish the publisher never sees moves the resident epoch out
        // from under the epoch it remembers
        SptNative.DbPublish(Encoding.UTF8.GetBytes("{\"schema\":1,\"roots\":{}}"));

        GenerateWave(_batcher);

        Assert.That(_batcher.LastSendIncludedViewsOverride, Is.False, "the stale-epoch miss should have republished and retried");
    }

    /// <summary>
    /// The flip's core promise, and the gate on every resident/override mapping choice — the
    /// handbook price values, the default-preset re-key and the exp table (the mod-pool slot order
    /// is out of scope — both arms take it from the one <c>BuildSharedVarying</c> call below, so no
    /// divergence is constructible there): the same seeded wave sent once off the resident DB and
    /// once with the C#-built views override must generate identical bots, compared as normalized
    /// JSON down to every field.
    /// PMC and non-PMC waves both run — only the PMC wave draws levels from the exp table.
    /// </summary>
    [Test]
    public void AResidentSendAndAnOverrideSendProduceIdenticalBotsFieldForField()
    {
        foreach (var isPmc in new[] { true, false })
        {
            var label = isPmc ? "pmc wave resident vs override" : "assault wave resident vs override";
            var request = BuildWaveRequest(isPmc, [Seed, Seed + 1, Seed + 2]);

            var residentResult = ResidentDbDispatch.Send(
                _publisher,
                epoch =>
                {
                    request.Epoch = epoch;
                    return SptNative.GenerateBotInventoryBatch(request);
                }
            );
            Assert.That(residentResult.Bots.All(envelope => envelope.Result is not null), Is.True, $"{label}: a resident bot failed");

            request.Epoch = 0;
            request.ViewsOverride = BotPayloadProjection.BuildViewsOverride(
                _botEquipmentModGenerator.PresetHelper,
                _botLootGenerator.HandbookHelper,
                _itemHelper,
                _botWeaponGenerator.GlobalTable,
                _botEquipmentModGenerator.ItemFilterService,
                _botConfig,
                _pmcConfig,
                _botWeaponGenerator.RepairConfig,
                request.Shared.TemplateVariants!.Select(variant => variant.LootPools)
            );
            var overrideResult = SptNative.GenerateBotInventoryBatch(request);

            LootJsonAssert.AssertEqual(Serialize(residentResult), Serialize(overrideResult), label, Seed);
        }
    }

    /// <summary>
    /// The bot family's twin of
    /// <see cref="LootResidentDbTests.AnInPlaceLootMultiplierAdjustmentReachesAResidentSend"/>: an
    /// unbarriered dictionary-indexer write into
    /// <c>BotConfig.Equipment[role].Randomisation[band].EquipmentMods</c> — no setter, no write
    /// barrier, no stamp move — still reaches the native side of a *resident* send. It gets there
    /// through the *template*: <c>FilterBotEquipment</c> bakes the clamped band into
    /// <c>BotChances.EquipmentModsChances</c> before projection
    /// (<c>BotEquipmentFilterService.cs:63,82</c>), and templates stay varying. The same cells now
    /// *also* ride the <c>liveEquipmentMods</c> overlay on the varying block, so this case is
    /// over-determined and does not discriminate between the two paths. That is all this case pins:
    /// it perturbs the config itself, generates two waves whose only difference is that
    /// perturbation, and asserts their outputs differ.
    ///
    /// It does not gate the second-bot feedback loop <c>ReplayRandomisationClamps</c> drives: its
    /// perturbation is the test's own, not a clamp; its wave is daytime; and its level-1 band has no
    /// <c>NighttimeChanges</c>, so no clamp is reachable here at all. That loop — bot 1's clamp
    /// being visible to bot 2 — is gated only by
    /// <see cref="ASecondNighttimeBotSeesTheFirstBotsClampsOnTheResidentPath"/>.
    /// </summary>
    [Test]
    public void AnInPlaceEquipmentModClampReachesAResidentSend()
    {
        Assert.That(_botConfig.Equipment.TryGetValue("pmc", out var pmcEquipConfig), Is.True, "no pmc equipment config in bot.json");
        var band = _botHelper.GetBotRandomizationDetails(1, pmcEquipConfig!);
        Assert.That(band?.EquipmentMods, Is.Not.Null.And.Not.Empty, "no pmc randomisation band with equipment mods covers level 1");

        var original = new Dictionary<string, double>(band!.EquipmentMods!);
        string allSlotsBlocked,
            allSlotsForced;
        try
        {
            allSlotsBlocked = GeneratePmcWaveOffTheResidentDb(band, 0);
            allSlotsForced = GeneratePmcWaveOffTheResidentDb(band, 100);
        }
        finally
        {
            foreach (var (slot, chance) in original)
            {
                band.EquipmentMods![slot] = chance;
            }
        }

        Assert.That(allSlotsForced, Is.Not.EqualTo(allSlotsBlocked), "the in-place equipment-mod clamp never reached the native side");
    }

    /// <summary>
    /// Clamps every equipment-mod chance in the band — exactly the write
    /// <c>ReplayRandomisationClamps</c> makes, a dictionary indexer assignment with no setter —
    /// then sends one seeded pmc wave off the resident DB and returns its normalized serialization.
    /// </summary>
    private string GeneratePmcWaveOffTheResidentDb(RandomisationDetails band, double chancePercent)
    {
        foreach (var slot in band.EquipmentMods!.Keys.ToList())
        {
            band.EquipmentMods[slot] = chancePercent;
        }

        var request = BuildWaveRequest(isPmc: true, [Seed, Seed + 1, Seed + 2]);
        var result = ResidentDbDispatch.Send(
            _publisher,
            epoch =>
            {
                request.Epoch = epoch;
                return SptNative.GenerateBotInventoryBatch(request);
            }
        );
        Assert.That(result.Bots.All(envelope => envelope.Result is not null), Is.True, "a resident pmc bot failed");

        return Serialize(result);
    }

    /// <summary>
    ///     The spec's second-bot gate (2026-08-26-botconfig-equipment-split-design.md): bot 1's
    ///     nighttime clamps are written into the live config through the dictionary indexer —
    ///     no barrier, no stamp move, no republish — and bot 2's *resident* send must still see
    ///     them, because they ride the liveEquipmentMods overlay. A frozen resident copy fails
    ///     here from bot 2 on.
    /// </summary>
    [Test]
    public void ASecondNighttimeBotSeesTheFirstBotsClampsOnTheResidentPath()
    {
        Assert.That(_botConfig.Equipment.TryGetValue("pmc", out var pmcEquipConfig), Is.True, "no pmc equipment config in bot.json");
        var band = _botHelper.GetBotRandomizationDetails(NightBotLevel, pmcEquipConfig!);
        Assert.That(
            band?.EquipmentMods,
            Is.Not.Null.And.Not.Empty,
            $"no pmc randomisation band with equipment mods covers level {NightBotLevel}"
        );
        Assert.That(
            band!.NighttimeChanges?.EquipmentModsModifiers,
            Does.ContainKey(NightSlot),
            $"the level-{NightBotLevel} pmc band no longer modifies {NightSlot} at night"
        );

        // Restored slot by slot, through the same unbarriered indexer the clamp uses - the clamp
        // never adds a key, so this puts the band back exactly. Both this fixture and BotParityTests
        // resolve BotConfig from one container, so a leaked clamp would break their non-vacuity
        // asserts and the neighbouring case above.
        var original = new Dictionary<string, double>(band.EquipmentMods!);
        var raidData = _profileActivityService.GetProfileActivityRaidData(_sessionId);
        var originalRaidConfiguration = raidData.RaidConfiguration;
        var originalForceLegacy = _botConfig.ForceLegacyBotGeneration;

        Dictionary<string, double> residentAfterFirst,
            residentAfterSecond,
            legacyAfterSecond;
        try
        {
            // Hydrating BotLootCacheService is a one-off cost, paid before the raid is installed so
            // the pre-warm generation cannot fire a clamp of its own
            _botConfig.ForceLegacyBotGeneration = true;
            GenerateSingleBot(_botInventoryGenerator, NighttimeUsecDetails(), "usec", LootGenerationPath.Legacy);

            // factory4_night is the one location IsNightTime answers true for without consulting the
            // wall clock, so this case does not silently stop testing anything at 6am
            raidData.RaidConfiguration = new GetRaidConfigurationRequestData
            {
                Location = "factory4_night",
                TimeVariant = DateTimeEnum.CURR,
            };

            (residentAfterFirst, residentAfterSecond) = GenerateTwoNighttimeBots(band, forceLegacy: false);

            RestoreEquipmentMods(band, original);
            (_, legacyAfterSecond) = GenerateTwoNighttimeBots(band, forceLegacy: true);
        }
        finally
        {
            raidData.RaidConfiguration = originalRaidConfiguration;
            _botConfig.ForceLegacyBotGeneration = originalForceLegacy;
            RestoreEquipmentMods(band, original);
        }

        // Without this the compounding assert below could pass on a band nothing ever clamped
        Assert.That(residentAfterFirst, Is.Not.EqualTo(original), "bot 1 fired no clamp, so this case proves nothing");
        Assert.That(
            residentAfterFirst[NightSlot],
            Is.EqualTo(original[NightSlot] + NightModifier),
            $"bot 1 did not clamp {NightSlot} off the shipped band value"
        );
        Assert.That(
            residentAfterSecond[NightSlot],
            Is.EqualTo(original[NightSlot] + (2 * NightModifier)),
            $"bot 2's resident send did not see bot 1's clamp - the liveEquipmentMods overlay is not reaching the merge"
        );
        Assert.That(residentAfterSecond, Is.EqualTo(legacyAfterSecond), "the resident path's clamp state diverged from the legacy path's");
    }

    // The shipped pmc 15-22 band: mod_nvg starts at 0 and nighttimeChanges adds 30 per bot, so two
    // bots compound to 60. Level 20 selects it - the 1-14 band has equipmentMods but no
    // nighttimeChanges, so no clamp is reachable below 15.
    private const int NightBotLevel = 20;
    private const string NightSlot = "mod_nvg";
    private const double NightModifier = 30;

    /// <summary>
    /// Two sequential nighttime pmc bots down one path, returning the live band's
    /// <c>EquipmentMods</c> after each. Single-bot dispatch only: the batcher declines the batch
    /// path for any wave that could write clamps (<c>BotWaveBatcher.cs</c>), so a batch-shaped copy
    /// of this would fire none.
    /// </summary>
    private (Dictionary<string, double> AfterFirst, Dictionary<string, double> AfterSecond) GenerateTwoNighttimeBots(
        RandomisationDetails band,
        bool forceLegacy
    )
    {
        _botConfig.ForceLegacyBotGeneration = forceLegacy;
        var expected = forceLegacy ? LootGenerationPath.Legacy : LootGenerationPath.Native;

        GenerateSingleBot(_botInventoryGenerator, NighttimeUsecDetails(), "usec", expected);
        AssertResidentSend(forceLegacy);
        var afterFirst = new Dictionary<string, double>(band.EquipmentMods!);

        GenerateSingleBot(_botInventoryGenerator, NighttimeUsecDetails(), "usec", expected);
        AssertResidentSend(forceLegacy);
        var afterSecond = new Dictionary<string, double>(band.EquipmentMods!);

        return (afterFirst, afterSecond);
    }

    /// <summary>
    /// The native leg has to be a *resident* send or it proves nothing about the overlay - an
    /// override send carries the whole equipment block and would compound either way.
    /// </summary>
    private void AssertResidentSend(bool forceLegacy)
    {
        if (!forceLegacy)
        {
            Assert.That(_botInventoryGenerator.LastSendIncludedViewsOverride, Is.False, "the nighttime bot did not take a resident send");
        }
    }

    /// <summary>
    /// The pmc usec bot at a level whose randomisation band carries <c>NighttimeChanges</c>. PMCs
    /// read their template from the side, not the role - usec.json.
    /// </summary>
    private static BotGenerationDetails NighttimeUsecDetails()
    {
        return new BotGenerationDetails
        {
            Role = "pmcUSEC",
            RoleLowercase = "pmcusec",
            Side = "Usec",
            BotDifficulty = "normal",
            GameVersion = "standard",
            BotLevel = NightBotLevel,
            IsPmc = true,
        };
    }

    private static void RestoreEquipmentMods(RandomisationDetails band, Dictionary<string, double> original)
    {
        foreach (var (slot, chance) in original)
        {
            band.EquipmentMods![slot] = chance;
        }
    }

    /// <summary>
    /// One wave's request body off the live database, shared verbatim by the resident and the
    /// override send. One template variant covers the whole level range — variant banding is the
    /// caller's optimisation, not part of the resident/override contract under test.
    /// </summary>
    private GenerateBotInventoryBatchRequest BuildWaveRequest(bool isPmc, IReadOnlyList<ulong?> seeds)
    {
        var details = new BotGenerationDetails
        {
            Role = isPmc ? "pmcUSEC" : "assault",
            RoleLowercase = isPmc ? "pmcusec" : "assault",
            Side = isPmc ? "Usec" : "Savage",
            BotDifficulty = "normal",
            GameVersion = "standard",
            BotLevel = 1,
            IsPmc = isPmc,
        };

        var template = _cloner.Clone(_botTable.Types[isPmc ? "usec" : "assault"])!;
        _botEquipmentFilterService.FilterBotEquipment(_sessionId, template, details);
        var lootPools = BotPayloadProjection.BuildLootPools(_botLootGenerator.BotLootCacheService, template, details, _pmcConfig);
        // Non-PMC bots never draw a level, so their wave is the single [1, 1] band with no inputs
        var levelMax = isPmc ? _botWeaponGenerator.GlobalTable.Configuration.Exp.Level.ExperienceTable.Length : 1;

        return new GenerateBotInventoryBatchRequest
        {
            Epoch = 0,
            Shared = BotPayloadProjection.BuildSharedVarying(
                _sessionId,
                _profileHelper,
                _profileActivityService,
                _weatherHelper,
                _botConfig,
                isPmc ? new LevelGenerationView { LevelMin = 1, LevelMax = levelMax } : null,
                [
                    new BotTemplateVariantView
                    {
                        LevelMin = 1,
                        LevelMax = levelMax,
                        Template = BotPayloadProjection.BuildTemplateView(template),
                        LootPools = lootPools,
                        Appearance = template.BotAppearance,
                        Health = template.BotHealth,
                        Skills = template.BotSkills,
                        ExperienceReward = template.BotExperience.Reward,
                    },
                ]
            ),
            Bots = [.. seeds.Select(seed => BotPayloadProjection.BuildBotSlice(new MongoId(), details, seed))],
        };
    }

    /// <summary>
    /// Every generated item carries a fresh <c>MongoId</c>, which never repeats between two calls;
    /// <c>LootIdNormalizer</c> rewrites them to positional placeholders, exactly as the parity
    /// suites compare.
    /// </summary>
    private static string Serialize(object value)
    {
        return LootIdNormalizer.Normalize(JsonSerializer.Serialize(value, JsonUtil.JsonSerializerOptionsNoIndent!));
    }

    private static BotWaveBatcher BuildBatcherWithMods(DI di, IReadOnlyList<SptMod> mods)
    {
        var constructor = typeof(BotWaveBatcher).GetConstructors().Single();

        var arguments = constructor
            .GetParameters()
            .Select(parameter =>
            {
                if (parameter.ParameterType == typeof(IReadOnlyList<SptMod>))
                {
                    return mods;
                }

                return di.GetService(parameter.ParameterType);
            })
            .ToArray();

        return (BotWaveBatcher)constructor.Invoke(arguments);
    }

    private static BotInventoryGenerator BuildGeneratorWithFrozenConstructor(DI di)
    {
        var constructor = typeof(BotInventoryGenerator).GetConstructors().MinBy(ctor => ctor.GetParameters().Length)!;

        var arguments = constructor.GetParameters().Select(parameter => di.GetService(parameter.ParameterType)).ToArray();

        return (BotInventoryGenerator)constructor.Invoke(arguments);
    }
}
