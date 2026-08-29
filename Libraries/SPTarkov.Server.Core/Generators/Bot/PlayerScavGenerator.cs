using System.Globalization;
using System.Reflection;
using HarmonyLib;
using Microsoft.Extensions.Logging;
using SPTarkov.Common.Models.Logging;
using SPTarkov.DI.Annotations;
using SPTarkov.Server.Core.Extensions;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Helpers;
using SPTarkov.Server.Core.Helpers.Bot;
using SPTarkov.Server.Core.Helpers.Items;
using SPTarkov.Server.Core.Helpers.Profile;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Mod;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native;
using SPTarkov.Server.Core.Native.Bot;
using SPTarkov.Server.Core.Native.Db;
using SPTarkov.Server.Core.Native.PlayerScav;
using SPTarkov.Server.Core.Servers;
using SPTarkov.Server.Core.Services;
using SPTarkov.Server.Core.Services.Bot;
using SPTarkov.Server.Core.Services.Commerce;
using SPTarkov.Server.Core.Services.Locales;
using SPTarkov.Server.Core.Services.Server;
using SPTarkov.Server.Core.Utils;
using SPTarkov.Server.Core.Utils.Cloners;

namespace SPTarkov.Server.Core.Generators.Bot;

/// <summary>
/// Player scav generation runs in <c>rust/spt-native</c> by default; the karma work that feeds the
/// C#-side loot pools stays here, everything else crosses. The full 4.1.2 C# implementation is
/// retained below as the legacy path - it is the frozen mod contract (constructor and protected
/// members are apicompat-gated against the 4.1.2 baseline) and runs instead of the native path when
/// a Harmony patch on any frozen member is detected, when a mod substituted this generator or
/// BotInventoryGenerator, when the frozen constructor built the instance or when
/// PlayerScavConfig.ForceLegacyPlayerScavGeneration is set, so mod hooks fire with genuine baseline
/// semantics.
/// </summary>
[Injectable]
public class PlayerScavGenerator(
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
    TimeUtil timeUtil
)
{
    private readonly PlayerScavNativeRequestBuilder? _requestBuilder;
    private readonly BotInventoryGenerator? _botInventoryGenerator;
    private readonly SeasonalEventService? _seasonalEventService;
    private readonly IReadOnlyList<SptMod>? _loadedMods;
    private readonly DbPublisher? _dbPublisher;

    /// <summary>
    ///     The constructor the container uses: the frozen 4.1.2 constructor plus the native seams.
    ///     Additive and apicompat-verified. The seasonal service is not a frozen parameter - the
    ///     native arm's christmas strip needs it.
    /// </summary>
    public PlayerScavGenerator(
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
        : this(
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
            timeUtil
        )
    {
        _requestBuilder = requestBuilder;
        _botInventoryGenerator = botInventoryGenerator;
        _seasonalEventService = seasonalEventService;
        _loadedMods = loadedMods;
        _dbPublisher = dbPublisher;
    }

    /// <summary>
    ///     Which implementation the most recent generation call ran - the spt-native path or the
    ///     retained 4.1.2 C# path. Test seam; also handy in a debugger.
    /// </summary>
    internal LootGenerationPath LastPathTaken { get; private set; }

    /// <summary>
    ///     Test-only seed forwarded onto every native request.
    /// </summary>
    internal ulong? NativeTestSeed { get; set; }

    /// <summary>
    ///     Whether the most recent native send carried the C#-built views override rather than
    ///     naming a resident-DB epoch. Test seam.
    /// </summary>
    internal bool LastSendIncludedViewsOverride { get; private set; }

    /// <summary>
    ///     Whether an override-less send off the resident DB is ever allowed: the services exist,
    ///     the kill switch is off, and either no mods are loaded or the user vouched their mods
    ///     don't write tables directly. A generator built without the epoch-protocol services has
    ///     neither and always sends the override.
    /// </summary>
    private bool ResidentDbEligible()
    {
        return ResidentDbDispatch.Eligible(
            _dbPublisher,
            _loadedMods?.Count,
            playerScavConfig.DisableNativeRequestCache,
            playerScavConfig.TrustNativeRequestCacheWithMods
        );
    }

    /// <summary>
    ///     The explicit frozen set (spec § Override contract) - NOT a whole-type sweep:
    ///     AdjustItemWeights, GetKarmaLimitValuesByKey, GetScavStats, GetScavLevel,
    ///     GetScavExperience and SetScavCooldownTimer run C#-side on both arms, and Generate is the
    ///     dispatcher. Read via reflection by PlayerScavHookLivenessTests (Weather precedent).
    /// </summary>
    private static readonly List<MethodBase> _hookableMembers =
    [
        .. new[]
        {
            typeof(PlayerScavGenerator).GetMethod(
                nameof(AddAdditionalLootToPlayerScavContainers),
                BindingFlags.Instance | BindingFlags.NonPublic
            ),
            typeof(PlayerScavGenerator).GetMethod(nameof(ConstructBotBaseTemplate), BindingFlags.Instance | BindingFlags.NonPublic),
            typeof(PlayerScavGenerator).GetMethod(
                nameof(AdjustBotTemplateWithKarmaSpecificSettings),
                BindingFlags.Instance | BindingFlags.NonPublic
            ),
            typeof(PlayerScavGenerator).GetMethod(nameof(AdjustEquipmentWeights), BindingFlags.Static | BindingFlags.NonPublic),
            typeof(PlayerScavGenerator).GetMethod(nameof(AdjustWeaponModWeights), BindingFlags.Static | BindingFlags.NonPublic),
            typeof(PlayerScavGenerator).GetMethod(nameof(BlacklistEquipment), BindingFlags.Static | BindingFlags.NonPublic),
            typeof(BotGenerator).GetMethod(nameof(BotGenerator.GeneratePlayerScav)),
            typeof(BotGenerator).GetMethod("GenerateBot", BindingFlags.Instance | BindingFlags.NonPublic),
            // Excluded from BotInventoryGenerator's own frozen set (on the bot path a patch on it
            // wraps whichever arm runs) - but the pscav native arm never calls it at all, so a
            // patch on it must flip here.
            typeof(BotInventoryGenerator).GetMethod(nameof(BotInventoryGenerator.GenerateInventory)),
        }.OfType<MethodBase>(),
    ];

    /// <summary>
    ///     The legacy path runs when the frozen 4.1.2 constructor built this instance (it has no
    ///     native seam to dispatch to), when forced by config, when the bot inventory family is
    ///     itself off its native path, when any of the frozen members carries a live Harmony patch,
    ///     or when a mod has substituted this generator or BotInventoryGenerator - running the
    ///     retained C# implementation is the only way those hooks and replacements can take effect
    ///     with real baseline semantics.
    /// </summary>
    private bool UseLegacyPath()
    {
        if (
            _requestBuilder is null
            || _botInventoryGenerator is null
            || _seasonalEventService is null
            || playerScavConfig.ForceLegacyPlayerScavGeneration
        )
        {
            return true;
        }

        // The export runs the bot family's internals: anything that de-natives bot inventory
        // de-natives the player scav with it (BotWaveBatcher.CanBatch precedent). The subclass
        // check is ours to make: a BotInventoryGenerator subclass overriding GenerateInventory is
        // bypassed on this arm, and BotInventoryGenerator.UseLegacyPath has no self-type check.
        if (_botInventoryGenerator.UseLegacyPath() || _botInventoryGenerator.GetType() != typeof(BotInventoryGenerator))
        {
            return true;
        }

        if (
            _hookableMembers.Any(member =>
                Harmony.GetPatchInfo(member) is { } patches
                && (
                    patches.Prefixes.Count > 0
                    || patches.Postfixes.Count > 0
                    || patches.Transpilers.Count > 0
                    || patches.Finalizers.Count > 0
                )
            )
        )
        {
            return true;
        }

        // A mod registered its own subclass with a higher TypePriority, so the container handed us
        // an implementation the native side does not have
        return GetType() != typeof(PlayerScavGenerator);
    }

    /// <summary>
    ///     Update a player profile to include a new player scav profile
    /// </summary>
    /// <param name="sessionID">session id to specify what profile is updated</param>
    /// <returns>profile object</returns>
    public PmcData Generate(MongoId sessionID)
    {
        // get karma level from profile
        var profile = saveServer.GetProfile(sessionID);
        var profileCharactersClone = cloner.Clone(profile.CharacterData);
        var pmcDataClone = cloner.Clone(profileCharactersClone.PmcData);
        var existingScavDataClone = cloner.Clone(profileCharactersClone.ScavData);

        var scavKarmaLevel = pmcDataClone.GetScavKarmaLevel();

        // use karma level to get correct karmaSettings
        if (
            !playerScavConfig.KarmaLevel.TryGetValue(scavKarmaLevel.ToString(CultureInfo.InvariantCulture), out var playerScavKarmaSettings)
        )
        {
            logger.Error(serverLocalisationService.GetText("scav-missing_karma_settings", scavKarmaLevel));
        }

        if (logger.IsLogEnabled(LogLevel.Debug))
        {
            logger.Debug($"Generated player scav load out with karma level: {scavKarmaLevel}");
        }

        PmcData scavData;
        if (UseLegacyPath())
        {
            LastPathTaken = LootGenerationPath.Legacy;
            scavData = GenerateScavLegacy(sessionID, playerScavKarmaSettings, pmcDataClone);
        }
        else
        {
            LastPathTaken = LootGenerationPath.Native;
            scavData = GenerateScavNative(sessionID, playerScavKarmaSettings, pmcDataClone);
        }

        // No need for cache data, clear up - scavData.Id is still the generated bot id here; the
        // metadata block below overwrites it, so the order is load-bearing
        botInventoryContainerService.ClearCache(scavData.Id.Value);

        // Remove cached bot loot cache now scav is generated
        botLootCacheService.ClearCache();

        // Add scav metadata
        scavData.Savage = null;
        scavData.Aid = pmcDataClone.Aid;
        scavData.TradersInfo = pmcDataClone.TradersInfo;
        scavData.Info.Settings = new();
        scavData.Info.Bans = [];
        scavData.Info.RegistrationDate = pmcDataClone.Info.RegistrationDate;
        scavData.Info.GameVersion = pmcDataClone.Info.GameVersion;
        scavData.Info.MemberCategory = MemberCategory.UniqueId;
        scavData.Info.LockedMoveCommands = true;
        scavData.Info.MainProfileNickname = pmcDataClone.Info.Nickname;
        scavData.RagfairInfo = pmcDataClone.RagfairInfo;
        scavData.UnlockedInfo = pmcDataClone.UnlockedInfo;

        // Persist previous scav data into new scav
        scavData.Id = existingScavDataClone.Id ?? pmcDataClone.Savage;
        scavData.SessionId = existingScavDataClone.SessionId ?? pmcDataClone.SessionId;
        scavData.Skills = existingScavDataClone.GetSkillsOrDefault();
        scavData.Stats = GetScavStats(existingScavDataClone);
        scavData.Info.Level = GetScavLevel(existingScavDataClone);
        scavData.Info.Experience = GetScavExperience(existingScavDataClone);
        scavData.Quests = existingScavDataClone.Quests ?? [];
        scavData.TaskConditionCounters = existingScavDataClone.TaskConditionCounters ?? new();
        scavData.Notes = existingScavDataClone.Notes ?? new Notes { DataNotes = [] };
        scavData.WishList = existingScavDataClone.WishList ?? new();
        scavData.Encyclopedia = pmcDataClone.Encyclopedia ?? new();
        scavData.Variables = existingScavDataClone.Variables ?? new();

        // Player scavs don't have a secure
        scavData = profileHelper.RemoveSecureContainer(scavData);

        // Set cooldown timer
        SetScavCooldownTimer(scavData, pmcDataClone);

        // Assign newly generated scav profile
        saveServer.GetProfile(sessionID).CharacterData.ScavData = scavData;

        return scavData;
    }

    /// <summary>
    ///     The retained 4.1.2 generation body: karma applied to a merged base template, the whole
    ///     bot built C#-side, additional loot added afterwards.
    /// </summary>
    private PmcData GenerateScavLegacy(MongoId sessionID, KarmaLevel playerScavKarmaSettings, PmcData pmcDataClone)
    {
        // Edit baseBotNode values
        var baseBotNode = ConstructBotBaseTemplate(playerScavKarmaSettings.BotTypeForLoot);

        AdjustBotTemplateWithKarmaSpecificSettings(playerScavKarmaSettings, baseBotNode);

        var scavData = botGenerator.GeneratePlayerScav(
            sessionID,
            playerScavKarmaSettings.BotTypeForLoot.ToLowerInvariant(),
            "easy",
            baseBotNode,
            pmcDataClone
        );

        // Add additional items to player scav as loot
        AddAdditionalLootToPlayerScavContainers(
            scavData.Id.Value,
            playerScavKarmaSettings.LootItemsToAddChancePercent,
            scavData,
            [EquipmentSlots.TacticalVest, EquipmentSlots.Pockets, EquipmentSlots.Backpack]
        );

        return scavData;
    }

    /// <summary>
    ///     The native arm: the karma pieces that feed the C#-side loot pool hydration run here,
    ///     against clones, and everything else - the karma modifiers, the equipment blacklist, the
    ///     inventory and the additional loot - crosses to spt-native in one call.
    /// </summary>
    private PmcData GenerateScavNative(MongoId sessionID, KarmaLevel playerScavKarmaSettings, PmcData pmcDataClone)
    {
        // GetBotTemplate returns the live db table entry - every caller in the repo clones it. The
        // shell's prelude must never hold a live reference (a future prelude write would silently
        // corrupt the in-memory DB for the process lifetime); one cold-path clone buys that off.
        // The loot template is cloned for the same reason: the `with` expression below is a shallow
        // record copy, so an un-cloned source would leave BotChances and friends aliasing the DB.
        var lootTemplate = cloner.Clone(botHelper.GetBotTemplate(playerScavKarmaSettings.BotTypeForLoot));
        var assaultTemplate = cloner.Clone(botHelper.GetBotTemplate("assault"));

        // The two karma pieces whose output feeds C#-side loot-pool hydration run here, against
        // clones, through the real methods so patches fire (spec § Seam): item limits on a cloned
        // generation, the strips on a cloned inventory. Everything else karma does moves native.
        var karmaGeneration = cloner.Clone(lootTemplate.BotGeneration);
        AdjustItemWeights(playerScavKarmaSettings.ItemLimits, karmaGeneration.Items);

        var strippedInventory = cloner.Clone(lootTemplate.BotInventory);
        // Mirrors the legacy prelude's guard shape exactly: unconditional christmas check outside,
        // gifter-role check nested inside
        if (!_seasonalEventService!.ChristmasEventEnabled())
        {
            if (playerScavKarmaSettings.BotTypeForLoot.ToLowerInvariant() != "gifter")
            {
                _seasonalEventService.RemoveChristmasItemsFromBotInventory(
                    strippedInventory,
                    playerScavKarmaSettings.BotTypeForLoot.ToLowerInvariant()
                );
            }
        }

        botGenerator.RemoveBlacklistedLootFromBotTemplateInternal(strippedInventory);

        var hydrationTemplate = lootTemplate with { BotGeneration = karmaGeneration, BotInventory = strippedInventory };

        return botGenerator.GeneratePlayerScavNative(
            sessionID,
            playerScavKarmaSettings.BotTypeForLoot.ToLowerInvariant(),
            "easy",
            assaultTemplate,
            pmcDataClone,
            (bot, details) =>
            {
                var request = _requestBuilder!.Build(
                    bot.Id.Value,
                    sessionID,
                    hydrationTemplate,
                    details,
                    playerScavKarmaSettings,
                    NativeTestSeed
                );
                BotInventoryResult result;
                if (ResidentDbEligible())
                {
                    LastSendIncludedViewsOverride = false;
                    result = ResidentDbDispatch.Send(
                        _dbPublisher!,
                        epoch =>
                        {
                            request.Epoch = epoch;
                            request.ViewsOverride = null;
                            return SptNative.GeneratePlayerScav(request);
                        }
                    );
                }
                else
                {
                    LastSendIncludedViewsOverride = true;
                    request.ViewsOverride = _requestBuilder.BuildViewsOverride(request.LootPools);
                    result = SptNative.GeneratePlayerScav(request);
                }

                _botInventoryGenerator!.ReplayRandomisationClamps(details, result.RandomisationClamps);
                return result.Inventory;
            }
        );
    }

    /// <summary>
    ///     Add items picked from `playerscav.lootItemsToAddChancePercent`
    /// </summary>
    /// <param name="botId">Bots unique identifier</param>
    /// <param name="possibleItemsToAdd">dict of tpl + % chance to be added</param>
    /// <param name="scavData"></param>
    /// <param name="containersToAddTo">Possible slotIds to add loot to</param>
    protected void AddAdditionalLootToPlayerScavContainers(
        MongoId botId,
        Dictionary<MongoId, double> possibleItemsToAdd,
        BotBase scavData,
        HashSet<EquipmentSlots> containersToAddTo
    )
    {
        foreach (var tpl in possibleItemsToAdd)
        {
            var shouldAdd = randomUtil.GetChance100(tpl.Value);
            if (!shouldAdd)
            {
                continue;
            }

            var itemResult = itemHelper.GetItem(tpl.Key);
            if (!itemResult.Key)
            {
                logger.Warning(serverLocalisationService.GetText("scav-unable_to_add_item_to_player_scav", tpl));
                continue;
            }

            var itemTemplate = itemResult.Value;
            var itemsToAdd = new List<Item>
            {
                new()
                {
                    Id = new MongoId(),
                    Template = itemTemplate.Id,
                    Upd = botGeneratorHelper.GenerateExtraPropertiesForItem(itemTemplate, "assault", true),
                },
            };

            var result = botGeneratorHelper.AddItemWithChildrenToEquipmentSlot(
                botId,
                containersToAddTo,
                itemsToAdd[0].Id,
                itemTemplate.Id,
                itemsToAdd,
                scavData.Inventory
            );

            if (result != ItemAddedResult.SUCCESS)
            {
                if (logger.IsLogEnabled(LogLevel.Debug))
                {
                    logger.Debug($"Unable to add keycard to bot. Reason: {result.ToString()}");
                }
            }
        }
    }

    /// <summary>
    ///     Get a baseBot template
    ///     If the parameter doesnt match "assault", take parts from the loot type and apply to the return bot template
    /// </summary>
    /// <param name="botTypeForLoot">bot type to use for inventory/chances</param>
    /// <returns>IBotType object</returns>
    protected BotType ConstructBotBaseTemplate(string botTypeForLoot)
    {
        const string baseScavType = "assault";
        var asssaultBase = cloner.Clone(botHelper.GetBotTemplate(baseScavType));

        // Loot bot is same as base bot, return base with no modification
        if (botTypeForLoot == baseScavType)
        {
            return asssaultBase;
        }

        var lootBase = cloner.Clone(botHelper.GetBotTemplate(botTypeForLoot));
        asssaultBase.BotInventory = lootBase.BotInventory;
        asssaultBase.BotChances = lootBase.BotChances;
        asssaultBase.BotGeneration = lootBase.BotGeneration;

        return asssaultBase;
    }

    /// <summary>
    ///     Adjust equipment/mod/item generation values based on scav karma levels
    /// </summary>
    /// <param name="karmaSettings">Values to modify the bot template with</param>
    /// <param name="baseBotNode">bot template to modify according to karma level settings</param>
    protected void AdjustBotTemplateWithKarmaSpecificSettings(KarmaLevel karmaSettings, BotType baseBotNode)
    {
        // Adjust equipment chance values
        AdjustEquipmentWeights(karmaSettings.Modifiers.Equipment, baseBotNode.BotChances.EquipmentChances);

        // Adjust mod chance values
        AdjustWeaponModWeights(karmaSettings.Modifiers.Mod, baseBotNode.BotChances.WeaponModsChances);

        // Adjust item spawn quantity values
        AdjustItemWeights(karmaSettings.ItemLimits, baseBotNode.BotGeneration.Items);

        // Blacklist equipment, keyed by equipment slot
        BlacklistEquipment(karmaSettings, baseBotNode);
    }

    protected static void AdjustEquipmentWeights(
        Dictionary<string, double> equipmentChangesToApply,
        Dictionary<string, double> botEquipmentChances
    )
    {
        foreach (var (equipmentSlot, chanceToAdd) in equipmentChangesToApply)
        {
            // Adjustment value zero, nothing to do
            if (chanceToAdd == 0)
            {
                continue;
            }

            // Try and add new key with value
            if (!botEquipmentChances.TryAdd(equipmentSlot, chanceToAdd))
            {
                // Unable to add new, update existing
                botEquipmentChances[equipmentSlot] += chanceToAdd;
            }
        }
    }

    /// <summary>
    /// Get a bots item type weightings based on the desired key
    /// </summary>
    /// <param name="key">e.g. "healing" / "looseLoot"</param>
    /// <param name="botItemWeights"></param>
    /// <returns>GenerationData</returns>
    protected GenerationData? GetKarmaLimitValuesByKey(string key, GenerationWeightingItems botItemWeights)
    {
        switch (key)
        {
            case "healing":
                return botItemWeights.Healing;
            case "drugs":
                return botItemWeights.Drugs;
            case "stims":
                return botItemWeights.Stims;
            case "looseLoot":
                return botItemWeights.LooseLoot;
            case "magazines":
                return botItemWeights.Magazines;
            case "grenades":
                return botItemWeights.Grenades;
            case "backpackLoot":
                return botItemWeights.BackpackLoot;
            case "drink":
                return botItemWeights.Drink;
            case "currency":
                return botItemWeights.Currency;
            case "pocketLoot":
                return botItemWeights.PocketLoot;
            case "vestLoot":
                return botItemWeights.VestLoot;
            case "specialItems":
                return botItemWeights.SpecialItems;
            default:
                logger.Error($"Subtype: {key} not found");
                return null;
        }
    }

    protected static void AdjustWeaponModWeights(Dictionary<string, double> modChangesToApply, Dictionary<string, double> weaponModChances)
    {
        foreach (var (modSlot, weight) in modChangesToApply)
        {
            // Adjustment value zero, nothing to do
            if (weight == 0)
            {
                continue;
            }

            if (modChangesToApply.TryGetValue(modSlot, out var value))
            {
                weaponModChances.TryAdd(modSlot, 0);
                weaponModChances[modSlot] += value;
            }
        }
    }

    protected void AdjustItemWeights(
        Dictionary<string, GenerationData> karmaSettingsItemLimits,
        GenerationWeightingItems? botGenerationItems
    )
    {
        foreach (var (subType, limitData) in karmaSettingsItemLimits)
        {
            var playerValues = GetKarmaLimitValuesByKey(subType, botGenerationItems);
            if (playerValues is null)
            {
                continue;
            }

            if (limitData.Weights is not null)
            {
                playerValues.Weights = limitData.Weights;
            }

            if (limitData.Whitelist is not null)
            {
                playerValues.Whitelist = limitData.Whitelist;
            }
        }
    }

    protected static void BlacklistEquipment(KarmaLevel karmaSettings, BotType baseBotNode)
    {
        foreach (var (slot, blacklist) in karmaSettings.EquipmentBlacklist)
        {
            if (!baseBotNode.BotInventory.Equipment.TryGetValue(slot, out var equipmentDict))
            {
                continue;
            }
            foreach (var itemToRemove in blacklist)
            {
                equipmentDict.Remove(itemToRemove);
            }
        }
    }

    protected Stats GetScavStats(PmcData scavProfile)
    {
        return scavProfile.Stats ?? profileHelper.GetDefaultCounters();
    }

    protected int GetScavLevel(PmcData scavProfile)
    {
        // Info can be null on initial account creation
        if (scavProfile.Info?.Level == null)
        {
            return 1;
        }

        return scavProfile.Info?.Level ?? 1;
    }

    protected int GetScavExperience(PmcData scavProfile)
    {
        // Info can be null on initial account creation
        if (scavProfile.Info?.Experience == null)
        {
            return 0;
        }

        return scavProfile.Info?.Experience ?? 0;
    }

    /// <summary>
    ///     Set cooldown till scav is playable
    ///     take into account scav cooldown bonus
    /// </summary>
    /// <param name="scavData">scav profile</param>
    /// <param name="pmcData">pmc profile</param>
    protected void SetScavCooldownTimer(PmcData scavData, PmcData pmcData)
    {
        // Get sum of all scav cooldown reduction timer bonuses
        var modifier = 1d + pmcData.Bonuses.Where(x => x.Type == BonusType.ScavCooldownTimer).Sum(bonus => (bonus?.Value ?? 1) / 100);

        var fenceInfo = fenceService.GetFenceInfo(pmcData);
        modifier *= fenceInfo.SavageCooldownModifier;

        // Make sure to apply ScavCooldownTimer bonus from Hideout if the player has it.
        var scavLockDuration = globalTable.Configuration.SavagePlayCooldown * modifier;

        var fullProfile = profileHelper.GetFullProfile(pmcData.SessionId.Value);
        if (fullProfile?.ProfileInfo?.Edition?.StartsWith(AccountTypes.SPT_DEVELOPER, StringComparison.OrdinalIgnoreCase) ?? false)
        {
            // Force lock duration to 10seconds for dev profiles
            scavLockDuration = 10;
        }

        if (scavData?.Info != null)
        {
            scavData.Info.SavageLockTime = Math.Round(timeUtil.GetTimeStamp() + (scavLockDuration));
        }
    }
}
