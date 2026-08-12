using NUnit.Framework;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Services.Items;

namespace UnitTests.Tests.Services;

[TestFixture]
public class ItemFilterServiceTests
{
    private ItemFilterService _itemFilterService = default!;
    private ItemConfig _itemConfig = default!;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _itemFilterService = di.GetService<ItemFilterService>();
        _itemConfig = di.GetService<ItemConfig>();
    }

    /// <summary>
    /// The cache is what <see cref="ItemFilterService.IsLootableItemBlacklisted"/> answers from, so
    /// anything reading the blacklist in bulk has to read it here and not off the config
    /// </summary>
    [Test]
    public void LootableItemBlacklistCacheHoldsTheConfigListPlusRuntimeAdditions()
    {
        // A generated id, so polluting this shared singleton cannot affect another fixture
        var modBlacklistedTpl = new MongoId();

        _itemFilterService.AddItemToLootableBlacklistCache([modBlacklistedTpl]);
        var blacklist = _itemFilterService.GetLootableItemBlacklistCache();

        Assert.Multiple(() =>
        {
            Assert.That(blacklist, Does.Contain(modBlacklistedTpl));
            Assert.That(blacklist, Is.SupersetOf(_itemConfig.LootableItemBlacklist));
            Assert.That(_itemFilterService.IsLootableItemBlacklisted(modBlacklistedTpl), Is.True);
        });
    }
}
