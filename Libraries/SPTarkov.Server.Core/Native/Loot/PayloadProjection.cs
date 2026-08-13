using System.Text.Json;
using Microsoft.Extensions.Logging;
using SPTarkov.Common.Models.Logging;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Services.Locales;

namespace SPTarkov.Server.Core.Native.Loot;

/// <summary>
/// The parts of building a native loot request and consuming its result that every generator does
/// the same way: projecting the items table into <see cref="ItemView"/>s, and replaying the log
/// lines the native side collected instead of writing itself.
/// </summary>
internal static class PayloadProjection
{
    /// <summary>
    /// Every projection a native generator reads off a <c>TemplateItem</c>, in one pass over the
    /// live items table. Templates without props are dropped - their absence is how the native side
    /// says "lacks _props".
    /// </summary>
    internal static Dictionary<MongoId, ItemView> BuildItemsView(Dictionary<MongoId, TemplateItem> items)
    {
        var itemsView = new Dictionary<MongoId, ItemView>(items.Count);

        foreach (var (tpl, template) in items)
        {
            var props = template.Properties;
            if (props is null)
            {
                continue;
            }

            var firstGrid = props.Grids?.FirstOrDefault();
            var firstStackSlot = props.StackSlots?.FirstOrDefault();
            var firstCartridgeSlot = props.Cartridges?.FirstOrDefault();
            var firstChamber = props.Chambers?.FirstOrDefault();
            var stackSlotFilter = firstStackSlot?.Properties?.Filters?.FirstOrDefault()?.Filter;

            itemsView[tpl] = new ItemView
            {
                // Cast needed on both arms: MongoId's implicit string conversion otherwise turns the
                // null arm into a default MongoId instead of leaving the member absent
                Parent = template.Parent.IsEmpty ? null : (MongoId?)template.Parent,
                Width = props.Width,
                Height = props.Height,
                StackMaxSize = props.StackMaxSize,
                StackMinRandom = props.StackMinRandom,
                StackMaxRandom = props.StackMaxRandom,
                ExtraSizeUp = props.ExtraSizeUp,
                ExtraSizeDown = props.ExtraSizeDown,
                ExtraSizeLeft = props.ExtraSizeLeft,
                ExtraSizeRight = props.ExtraSizeRight,
                ExtraSizeForceAdd = props.ExtraSizeForceAdd,
                GridCellsH = firstGrid?.Properties?.CellsH,
                GridCellsV = firstGrid?.Properties?.CellsV,
                StackSlotMaxCount = firstStackSlot?.MaxCount,
                // Deliberate divergence: an empty filter set is sent as null rather than as the
                // empty MongoId `Filter?.FirstOrDefault()` would have produced. Never fires on
                // vanilla data - a stack slot with an empty filter has nothing to stack
                StackSlotFirstFilterFirst = stackSlotFilter is { Count: > 0 } ? (MongoId?)stackSlotFilter.First() : null,
                CartridgesMaxCount = firstCartridgeSlot?.MaxCount,
                CartridgesFirstFilter = firstCartridgeSlot?.Properties?.Filters?.FirstOrDefault()?.Filter,
                ChambersFirstFilter = firstChamber?.Properties?.Filters?.FirstOrDefault()?.Filter,
                Slots = ToSlotViews(props.Slots),
                // Projected verbatim - an empty chamber list is not the same as no chamber list
                Chambers = ToSlotViews(props.Chambers),
                Cartridges = ToSlotViews(props.Cartridges),
                ConflictingItems = props.ConflictingItems,
                Caliber = props.Caliber,
                AmmoCaliber = props.AmmoCaliber,
                DefAmmo = props.DefAmmo,
                Name = template.Name,
                Type = template.Type,
                ArmorClass = props.ArmorClass,
                // Not coalesced: the reward pool filters on `false` and the sealed container pool on
                // `null`, so the two have to stay distinguishable
                QuestItem = props.QuestItem,
                // Enum member names, not their numeric values - the native side string-compares them
                ReloadMode = props.ReloadMode?.ToString(),
                ReloadMagType = props.ReloadMagType?.ToString(),
                IsChamberLoad = props.IsChamberLoad,
                DefMagType = props.DefMagType,
                LinkedWeapon = props.LinkedWeapon,
                MaxDurability = props.MaxDurability,
                WeapClass = props.WeapClass,
                HasHinge = props.HasHinge,
                Foldable = props.Foldable,
                FoldedSlot = props.FoldedSlot,
                SizeReduceRight = props.SizeReduceRight,
                WeapFireType = props.WeapFireType,
                MaxHpResource = props.MaxHpResource,
                MaxResource = props.MaxResource,
                FoodUseTime = props.FoodUseTime,
                FaceShieldComponent = props.FaceShieldComponent,
                BlocksEarpiece = props.BlocksEarpiece,
                BlocksEyewear = props.BlocksEyewear,
                BlocksFaceCover = props.BlocksFaceCover,
                BlocksHeadwear = props.BlocksHeadwear,
                BlocksFolding = props.BlocksFolding,
                BlocksCollapsible = props.BlocksCollapsible,
                BlockLeftStance = props.BlockLeftStance,
                BlocksArmorVest = props.BlocksArmorVest,
                Grids = props
                    .Grids?.Select(grid => new GridView
                    {
                        Name = grid.Name,
                        CellsH = grid.Properties?.CellsH,
                        CellsV = grid.Properties?.CellsV,
                        Filters = grid
                            .Properties?.Filters?.Select(filter => new GridFilterView
                            {
                                Filter = filter.Filter,
                                ExcludedFilter = filter.ExcludedFilter,
                            })
                            .ToList(),
                    })
                    .ToList(),
                Durability = props.Durability,
                MaximumNumberOfUsage = props.MaximumNumberOfUsage,
                MaxRepairResource = props.MaxRepairResource,
                CanSellOnRagfair = props.CanSellOnRagfair,
            };
        }

        return itemsView;
    }

    private static List<SlotView>? ToSlotViews(IEnumerable<Slot>? slots)
    {
        return slots
            ?.Select(slot => new SlotView
            {
                Name = slot.Name,
                Required = slot.Required,
                Filter = slot.Properties?.Filters?.FirstOrDefault()?.Filter,
                Plate = slot.Properties?.Filters?.FirstOrDefault()?.Plate,
            })
            .ToList();
    }

    /// <summary>
    /// Write out the log lines the native generator collected instead of logging itself, so the
    /// server log reads as it did before the cutover
    /// </summary>
    internal static void ReplayDiagnostics<T>(
        List<Diagnostic> diagnostics,
        ISptLogger<T> logger,
        ServerLocalisationService serverLocalisationService
    )
    {
        foreach (var diagnostic in diagnostics)
        {
            if (diagnostic.Level == "debug" && !logger.IsLogEnabled(LogLevel.Debug))
            {
                continue;
            }

            var message = LocaliseDiagnostic(diagnostic, serverLocalisationService);
            switch (diagnostic.Level)
            {
                case "debug":
                    logger.Debug(message);
                    break;
                case "warning":
                    logger.Warning(message);
                    break;
                case "error":
                    logger.Error(message);
                    break;
                case "success":
                    logger.Success(message);
                    break;
                default:
                    // Never drop a line a future native version tags with a level we don't know
                    logger.Warning($"[{diagnostic.Level}] {message}");
                    break;
            }
        }
    }

    private static string LocaliseDiagnostic(Diagnostic diagnostic, ServerLocalisationService serverLocalisationService)
    {
        if (diagnostic.LocaleKey is null)
        {
            return diagnostic.Message ?? string.Empty;
        }

        if (diagnostic.Args is not { } args)
        {
            return serverLocalisationService.GetText(diagnostic.LocaleKey);
        }

        // A scalar argument is the `%s` overload
        if (args.ValueKind != JsonValueKind.Object)
        {
            return serverLocalisationService.GetText(diagnostic.LocaleKey, args.ToString());
        }

        // Named arguments are substituted here rather than by ServerLocalisationService: it reads its
        // args object's *properties* by reflection, which only works for the anonymous types the C#
        // call sites passed - a dictionary would leave every `{{placeholder}}` in place
        var text = serverLocalisationService.GetText(diagnostic.LocaleKey);
        foreach (var argument in args.EnumerateObject())
        {
            text = text.Replace($"{{{{{argument.Name}}}}}", argument.Value.ToString());
        }

        return text;
    }
}
