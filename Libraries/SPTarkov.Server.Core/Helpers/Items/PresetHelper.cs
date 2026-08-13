using SPTarkov.DI.Annotations;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Presets;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Utils.Cloners;

namespace SPTarkov.Server.Core.Helpers.Items;

[Injectable(InjectionType.Singleton)]
public class PresetHelper(GlobalTable globalTable, ItemHelper itemHelper, ICloner cloner)
{
    protected Dictionary<MongoId, Preset>? DefaultEquipmentPresets;
    protected Dictionary<MongoId, Preset>? DefaultWeaponPresets;

    /// <summary>
    ///     Preset cache - key = item tpl, value = preset ids
    /// </summary>
    protected Dictionary<MongoId, PresetCacheDetails> PresetCache = new();

    public void HydratePresetStore(Dictionary<MongoId, PresetCacheDetails> input)
    {
        PresetCache = input;
    }

    /// <summary>
    /// Get weapon and armor default presets, keyed to preset id NOT item tpl
    /// </summary>
    /// <returns></returns>
    public Dictionary<MongoId, Preset> GetDefaultPresets()
    {
        var weapons = GetDefaultWeaponPresets();
        var equipment = GetDefaultEquipmentPresets();

        return weapons.UnionBy(equipment, kvp => kvp.Key).ToDictionary(kvp => kvp.Key, kvp => kvp.Value);
    }

    /// <summary>
    /// Get weapon and armor default presets, keyed to root items tpl
    /// </summary>
    /// <returns>dictionary of presets keyed by the root items tpl</returns>
    public Dictionary<MongoId, Preset> GetDefaultPresetsByTplKey()
    {
        // Weapons and equipment keyed by their preset id
        var weapons = GetDefaultWeaponPresets().Values;
        var equipment = GetDefaultEquipmentPresets().Values;

        return weapons
            .Concat(equipment)
            .Where(preset => preset.Items.Count > 0) // Some safety to prevent nullref
            .ToDictionary(preset => preset.Items.FirstOrDefault()!.Template);
    }

    /// <summary>
    /// Get the default preset of every tpl that has one, resolved exactly as
    /// <see cref="GetDefaultPreset"/> resolves a single tpl: the default-flagged preset when it is a
    /// weapon or equipment default, otherwise the first preset the tpl has. Presets are not cloned
    /// </summary>
    /// <returns>Presets keyed by the tpl they are the default for</returns>
    public Dictionary<MongoId, Preset> GetDefaultPresetByTpl()
    {
        // Fills the two caches GetDefaultPreset reads, so a tpl is never pushed down the fallback
        // path just because nothing has asked for the defaults yet
        var weaponPresets = GetDefaultWeaponPresets();
        var equipmentPresets = GetDefaultEquipmentPresets();

        var defaultPresets = new Dictionary<MongoId, Preset>(PresetCache.Count);
        foreach (var (templateId, presetDetails) in PresetCache)
        {
            if (presetDetails.DefaultId is null)
            {
                continue;
            }

            if (
                !weaponPresets.TryGetValue(presetDetails.DefaultId.Value, out var defaultPreset)
                && !equipmentPresets.TryGetValue(presetDetails.DefaultId.Value, out defaultPreset)
            )
            {
                // Default not found in weapon or equipment, use first preset in list
                defaultPreset = globalTable.ItemPresets[presetDetails.PresetIds.First()];
            }

            defaultPresets[templateId] = defaultPreset;
        }

        return defaultPresets;
    }

    /// <summary>
    /// Get default weapon presets
    /// </summary>
    /// <returns></returns>
    public Dictionary<MongoId, Preset> GetDefaultWeaponPresets()
    {
        if (DefaultWeaponPresets is null)
        {
            var tempPresets = globalTable.ItemPresets;
            DefaultWeaponPresets = tempPresets
                .Where(p => p.Value.Encyclopedia != null && itemHelper.IsOfBaseclass(p.Value.Encyclopedia.Value, BaseClasses.WEAPON))
                .ToDictionary();
        }

        return DefaultWeaponPresets;
    }

    /// <summary>
    /// Get default equipment presets
    /// </summary>
    /// <returns>Dictionary</returns>
    public Dictionary<MongoId, Preset> GetDefaultEquipmentPresets()
    {
        if (DefaultEquipmentPresets == null)
        {
            var tempPresets = globalTable.ItemPresets;
            DefaultEquipmentPresets = tempPresets
                .Where(p => p.Value.Encyclopedia != null && itemHelper.ArmorItemCanHoldMods(p.Value.Encyclopedia.Value))
                .ToDictionary();
        }

        return DefaultEquipmentPresets;
    }

    /// <summary>
    /// Is the provided id a preset id
    /// </summary>
    /// <param name="id">Value to check</param>
    /// <returns>True = preset exists for this id</returns>
    public bool IsPreset(MongoId id)
    {
        if (id.IsEmpty)
        {
            return false;
        }

        return globalTable.ItemPresets.ContainsKey(id);
    }

    /// <summary>
    /// Checks to see if the preset is of the given base class
    /// </summary>
    /// <param name="id">id of the preset</param>
    /// <param name="baseClass">BaseClasses enum to check against</param>
    /// <returns>True if the preset is of the given base class, false otherwise</returns>
    public bool IsPresetBaseClass(MongoId id, MongoId baseClass)
    {
        return IsPreset(id) && itemHelper.IsOfBaseclass(GetPreset(id)!.Encyclopedia!.Value, baseClass);
    }

    /// <summary>
    /// Does the provided tpl have a preset
    /// </summary>
    /// <param name="templateId">Tpl id to check</param>
    /// <returns>True if preset exists for tpl</returns>
    public bool HasPreset(MongoId templateId)
    {
        return PresetCache.ContainsKey(templateId);
    }

    /// <summary>
    /// Every tpl <see cref="HasPreset"/> answers true for, so a caller that would otherwise probe it
    /// per tpl can hand the whole set over the native boundary at once
    /// </summary>
    /// <returns>The preset cache's tpl keys</returns>
    internal IReadOnlyCollection<MongoId> GetTplsWithPresets()
    {
        return PresetCache.Keys;
    }

    /// <summary>
    /// The globals' preset map, keyed exactly as <see cref="IsPreset"/> and <see cref="GetPreset"/>
    /// key it, so a caller handing the whole map over a boundary keeps that key domain. Not cloned -
    /// unlike <see cref="GetAllPresets"/>, which also drops the keys
    /// </summary>
    /// <returns>Presets keyed by preset id</returns>
    internal IReadOnlyDictionary<MongoId, Preset> GetPresetsByPresetId()
    {
        return globalTable.ItemPresets;
    }

    public Preset? GetPreset(MongoId id)
    {
        return cloner.Clone(globalTable.ItemPresets[id]);
    }

    /// <summary>
    /// Get all presets from globals db
    /// </summary>
    /// <returns>List</returns>
    public List<Preset>? GetAllPresets()
    {
        return cloner.Clone(globalTable.ItemPresets.Values.ToList());
    }

    /// <summary>
    ///     Get a clone of a tpls presets
    /// </summary>
    /// <param name="templateId">Tpl to get presets for</param>
    /// <returns>List</returns>
    public List<Preset>? GetPresets(MongoId templateId)
    {
        // Try and get preset ids from cache if they exist
        if (!PresetCache.TryGetValue(templateId, out var presetDetailsForTpl))
        {
            // None found, early exit
            return [];
        }

        // Use gathered preset ids to get full preset objects, clone and return
        return cloner.Clone(presetDetailsForTpl.PresetIds.Select(x => globalTable.ItemPresets[x]).ToList());
    }

    /// <summary>
    ///     Get a cloned default preset for passed in item tpl
    /// </summary>
    /// <param name="templateId">Items tpl to get preset for</param>
    /// <returns>null if no default preset, otherwise Preset</returns>
    public Preset? GetDefaultPreset(MongoId templateId)
    {
        // look in main cache for presets for this tpl
        if (!PresetCache.TryGetValue(templateId, out var presetDetails))
        {
            return null;
        }

        if (presetDetails.DefaultId is null)
        {
            return null;
        }

        // Use default preset id from above cache to find the weapon/equipment preset
        if (DefaultWeaponPresets?.TryGetValue(presetDetails.DefaultId.Value, out var defaultPreset) is null or false)
        {
            if (DefaultEquipmentPresets?.TryGetValue(presetDetails.DefaultId.Value, out defaultPreset) is null or false)
            {
                // Default not found in weapon or equipment, return first preset in list
                return cloner.Clone(globalTable.ItemPresets[presetDetails.PresetIds.First()]);
            }
        }

        return cloner.Clone(defaultPreset);
    }

    /// <summary>
    ///     Get the presets root item tpl
    /// </summary>
    /// <param name="presetId">Preset id to look up</param>
    /// <returns>tpl mongoid</returns>
    public MongoId GetBaseItemTpl(MongoId presetId)
    {
        if (!globalTable.ItemPresets.TryGetValue(presetId, out var preset))
        {
            // No preset exists
            return "";
        }

        var rootItem = preset.Items.FirstOrDefault(x => x.Id == preset.Parent);
        if (rootItem is null)
        {
            // Cant find root item
            return "";
        }

        return rootItem.Template;
    }

    /// <summary>
    /// Return the price of the preset for the given item tpl, or for the tpl itself if no preset exists
    /// </summary>
    /// <param name="tpl">The item template to get the price of</param>
    /// <returns>The price of the given item preset, or base item if no preset exists</returns>
    public double GetDefaultPresetOrItemPrice(MongoId tpl)
    {
        // Get default preset if it exists
        var defaultPreset = GetDefaultPreset(tpl);

        // Bundle up tpls we want price for
        var tpls = defaultPreset is not null ? defaultPreset.Items.Select(item => item.Template) : [tpl];

        // Get price of tpls
        return itemHelper.GetItemAndChildrenPrice(tpls);
    }
}
