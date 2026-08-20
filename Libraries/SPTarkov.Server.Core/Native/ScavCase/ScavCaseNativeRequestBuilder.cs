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
    /// <summary>
    /// The full override send: an epoch-0 envelope carrying both halves. What every native call
    /// paid before the resident-DB flip - kept whole so the request-build benchmark keeps measuring
    /// exactly that.
    /// </summary>
    public ScavCaseRewardsRequest Build(MongoId recipeId, ulong? testSeed)
    {
        return new ScavCaseRewardsRequest
        {
            Epoch = 0,
            ViewsOverride = BuildViewsOverride(),
            Varying = BuildVarying(recipeId, testSeed),
        };
    }

    /// <summary>
    /// The per-request and service-backed half, riding every send.
    /// </summary>
    public ScavCaseVarying BuildVarying(MongoId recipeId, ulong? testSeed)
    {
        return new ScavCaseVarying
        {
            RecipeId = recipeId,
            InactiveSeasonalItems = seasonalEventService.GetInactiveSeasonalEventItems(),
            GlobalBlacklist = itemFilterService.GetItemBlacklistCache(),
            TestSeed = testSeed,
        };
    }

    /// <summary>
    /// The database views and config-backed inputs an override send carries - the distrust
    /// fallback, built fresh per call so a mod that swaps an item, blacklists one or edits the
    /// config at runtime is picked up. On the resident arm these come off the published
    /// <c>configs</c> root's <c>spt-scavcase</c>/<c>spt-item</c> stems instead.
    /// </summary>
    public ScavCaseViewsOverride BuildViewsOverride()
    {
        var itemsView = PayloadProjection.BuildItemsView(templateTable.Items);

        // Every tpl, zeros included: a handbook miss is 0 here and a missing key is 0 on the native
        // side, so filtering them would only trade payload bytes for a divergence risk
        var staticPrices = new Dictionary<MongoId, double>(itemsView.Count);
        foreach (var tpl in itemsView.Keys)
        {
            staticPrices[tpl] = ragfairPriceService.GetStaticPriceForItem(tpl) ?? 0;
        }

        return new ScavCaseViewsOverride
        {
            ScavRecipes = BuildRecipeViews(),
            ItemsView = itemsView,
            StaticPrices = staticPrices,
            DefaultPresetsByTpl = presetHelper
                .GetDefaultPresetByTpl()
                .ToDictionary(preset => preset.Key, preset => PayloadProjection.ToPresetView(preset.Value)),
            Config = scavCaseConfig,
            RewardItemBlacklist = itemFilterService.GetItemRewardBlacklist(),
            BossItems = itemFilterService.GetBossItems(),
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
