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
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Spt.Bots;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native.Bot;
using SPTarkov.Server.Core.Servers;
using SPTarkov.Server.Core.Services.Bot;
using SPTarkov.Server.Core.Services.Profile;
using SPTarkov.Server.Core.Utils;
using SPTarkov.Server.Core.Utils.Cloners;
using ProfileInfo = SPTarkov.Server.Core.Models.Eft.Profile.Info;

namespace UnitTests.Tests.Generators;

/// <summary>
/// The native bot request is rebuilt, serialised and re-parsed for every single bot, and ~92% of a
/// bot's wall clock is that round trip - so a byte on this wire is paid for per bot, not per raid.
/// These tests pin the two places the payload carried the same presets more than once.
/// </summary>
[TestFixture]
[NonParallelizable]
public class BotPayloadSizeTests
{
    private BotWeaponGenerator _botWeaponGenerator = default!;
    private BotLootGenerator _botLootGenerator = default!;
    private BotEquipmentModGenerator _botEquipmentModGenerator = default!;
    private BotGeneratorHelper _botGeneratorHelper = default!;
    private ProfileHelper _profileHelper = default!;
    private ItemHelper _itemHelper = default!;
    private WeatherHelper _weatherHelper = default!;
    private ProfileActivityService _profileActivityService = default!;
    private BotEquipmentFilterService _botEquipmentFilterService = default!;
    private BotConfig _botConfig = default!;
    private PmcConfig _pmcConfig = default!;
    private BotTable _botTable = default!;
    private ICloner _cloner = default!;

    private MongoId _sessionId;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _botWeaponGenerator = di.GetService<BotWeaponGenerator>();
        _botLootGenerator = di.GetService<BotLootGenerator>();
        _botEquipmentModGenerator = di.GetService<BotEquipmentModGenerator>();
        _botGeneratorHelper = di.GetService<BotGeneratorHelper>();
        _profileHelper = di.GetService<ProfileHelper>();
        _itemHelper = di.GetService<ItemHelper>();
        _weatherHelper = di.GetService<WeatherHelper>();
        _profileActivityService = di.GetService<ProfileActivityService>();
        _botEquipmentFilterService = di.GetService<BotEquipmentFilterService>();
        _botConfig = di.GetService<BotConfig>();
        _pmcConfig = di.GetService<PmcConfig>();
        _botTable = di.GetService<BotTable>();
        _cloner = di.GetService<ICloner>();

        _sessionId = new MongoId();
        di.GetService<SaveServer>().CreateProfile(new ProfileInfo { ProfileId = _sessionId });
    }

    /// <summary>
    /// <c>itemPresets</c> and <c>presetsById</c> were projected from one dictionary and serialised
    /// twice - the same ~0.47 MiB on the wire in both, about 9% of the request.
    /// </summary>
    [Test]
    public void PresetsAreSentOnce()
    {
        using var request = JsonDocument.Parse(SerializeRequest());

        Assert.That(
            request.RootElement.TryGetProperty("presetsById", out _),
            Is.False,
            "presetsById duplicates itemPresets verbatim; the native side should read one map"
        );
    }

    /// <summary>
    /// Every default preset is an entry of <c>itemPresets</c> - <c>PresetHelper</c> resolves them
    /// out of <c>globalTable.ItemPresets</c> - so the tpl only needs to carry the preset's id.
    /// </summary>
    [Test]
    public void DefaultPresetsAreSentAsIdsNotWholePresets()
    {
        using var request = JsonDocument.Parse(SerializeRequest());

        var defaults = request.RootElement.GetProperty("defaultPresetsByTpl");
        Assert.That(defaults.EnumerateObject().Any(), Is.True, "fixture needs a database with default presets");

        foreach (var entry in defaults.EnumerateObject())
        {
            Assert.That(
                entry.Value.ValueKind,
                Is.EqualTo(JsonValueKind.String),
                $"default preset for {entry.Name} should be the preset id, resolved against itemPresets"
            );
        }
    }

    /// <summary>
    /// Both fixes together, as the thing they are actually for: bytes crossing the FFI per bot.
    /// Sending the presets twice and inlining the defaults cost 766,817 bytes of a 5,583,351-byte
    /// request; the budget is the 4,816,534 that leaves, plus headroom for database churn. It is a
    /// regression guard, so re-baseline it deliberately - never widen it to make a diff pass.
    /// </summary>
    [Test]
    public void RequestStaysUnderTheWireBudget()
    {
        const int budgetBytes = 4_900_000;

        Assert.That(
            SerializeRequest().Length,
            Is.LessThan(budgetBytes),
            "the bot request is rebuilt and re-parsed per bot; a regression here is paid on every spawn"
        );
    }

    private byte[] SerializeRequest()
    {
        var details = new BotGenerationDetails
        {
            Role = "pmcUSEC",
            RoleLowercase = "pmcusec",
            Side = "Usec",
            BotDifficulty = "normal",
            GameVersion = "standard",
            BotLevel = 1,
            IsPmc = true,
        };

        var template = _cloner.Clone(_botTable.Types["usec"])!;
        _botEquipmentFilterService.FilterBotEquipment(_sessionId, template, details);

        var request = BotPayloadProjection.BuildRequest(
            new MongoId(),
            _sessionId,
            template,
            details,
            null,
            _profileHelper,
            _profileActivityService,
            _weatherHelper,
            _botGeneratorHelper,
            _botEquipmentFilterService,
            _botLootGenerator.BotLootCacheService,
            _botEquipmentModGenerator.PresetHelper,
            _botEquipmentModGenerator.ItemFilterService,
            _botLootGenerator.HandbookHelper,
            _itemHelper,
            _botWeaponGenerator.GlobalTable,
            _botConfig,
            _pmcConfig,
            _botWeaponGenerator.RepairConfig
        );

        return JsonSerializer.SerializeToUtf8Bytes(request, JsonUtil.JsonSerializerOptionsNoIndent!);
    }
}
