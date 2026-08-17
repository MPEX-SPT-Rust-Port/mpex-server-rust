using SPTarkov.DI.Annotations;
using SPTarkov.Server.Core.Helpers.Items;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native.Loot;
using SPTarkov.Server.Core.Services.Items;
using SPTarkov.Server.Core.Services.Ragfair;
using SPTarkov.Server.Core.Services.Server;

namespace SPTarkov.Server.Core.Native.ScavCase;

/// <summary>
/// Assembles the <see cref="ScavCaseRewardsRequest"/> for one scav case generation out of the live
/// database, services and config - everything <c>ScavCaseRewardGenerator</c> would have read for
/// itself.
/// </summary>
[Injectable]
public class ScavCaseNativeRequestBuilder(
    HideoutTable hideoutTable,
    TemplateTable templateTable,
    PresetHelper presetHelper,
    RagfairPriceService ragfairPriceService,
    ItemFilterService itemFilterService,
    SeasonalEventService seasonalEventService,
    ScavCaseConfig scavCaseConfig
)
{
    public ScavCaseRewardsRequest Build(MongoId recipeId, ulong? testSeed)
    {
        var itemsView = PayloadProjection.BuildItemsView(templateTable.Items);

        // Every tpl, zeros included: a handbook miss is 0 here and a missing key is 0 on the native
        // side, so filtering them would only trade payload bytes for a divergence risk
        var staticPrices = new Dictionary<MongoId, double>(itemsView.Count);
        foreach (var tpl in itemsView.Keys)
        {
            staticPrices[tpl] = ragfairPriceService.GetStaticPriceForItem(tpl) ?? 0;
        }

        return new ScavCaseRewardsRequest
        {
            RecipeId = recipeId,
            ScavRecipes = BuildRecipeViews(),
            Config = scavCaseConfig,
            ItemsView = itemsView,
            StaticPrices = staticPrices,
            DefaultPresetsByTpl = presetHelper
                .GetDefaultPresetByTpl()
                .ToDictionary(preset => preset.Key, preset => PayloadProjection.ToPresetView(preset.Value)),
            InactiveSeasonalItems = seasonalEventService.GetInactiveSeasonalEventItems(),
            GlobalBlacklist = itemFilterService.GetItemBlacklistCache(),
            RewardItemBlacklist = itemFilterService.GetItemRewardBlacklist(),
            BossItems = itemFilterService.GetBossItems(),
            TestSeed = testSeed,
        };
    }

    /// <summary>
    /// The recipe table under the native side's names. A recipe missing any of the three end product
    /// ranges is dropped: C# threw an NRE the moment it dereferenced one, so the recipe was never
    /// generatable, and sending a null would fail the parse of the whole request rather than just
    /// that recipe. Never fires on vanilla data.
    /// </summary>
    private List<ScavCaseRecipeView> BuildRecipeViews()
    {
        var views = new List<ScavCaseRecipeView>();

        foreach (var recipe in hideoutTable.Production.ScavRecipes ?? [])
        {
            if (recipe.EndProducts is not { Common: { } common, Rare: { } rare, Superrare: { } superrare })
            {
                continue;
            }

            views.Add(
                new ScavCaseRecipeView
                {
                    Id = recipe.Id,
                    EndProducts = new ScavCaseEndProductsView
                    {
                        Common = common,
                        Rare = rare,
                        Superrare = superrare,
                    },
                }
            );
        }

        return views;
    }
}
