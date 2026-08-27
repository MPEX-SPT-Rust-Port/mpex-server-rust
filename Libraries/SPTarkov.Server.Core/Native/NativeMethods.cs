using System.Runtime.InteropServices;

namespace SPTarkov.Server.Core.Native;

internal static unsafe partial class NativeMethods
{
    private const string LibraryName = "spt_native";

    // When a prepatcher mod is installed, PrepatchLoadContext loads the rewritten Core via
    // LoadFromStream, so the assembly has no location and default P/Invoke probing never checks
    // the app directory where libspt_native lives. Probe it explicitly; returning IntPtr.Zero
    // falls back to the default search.
    static NativeMethods()
    {
        NativeLibrary.SetDllImportResolver(
            typeof(NativeMethods).Assembly,
            (name, _, _) =>
            {
                if (name != LibraryName)
                {
                    return IntPtr.Zero;
                }

                // Phase 6b: mpex-server links spt-native as an rlib, so the exports live in the
                // executable itself and the resident DB's statics live in the host process.
                // GetMainProgramHandle is dlopen(NULL) - it probes the process's global symbol
                // scope, not the executable alone - but .NET loads native libraries RTLD_LOCAL, so
                // a cdylib loaded by the arm below can never answer here. A process started any
                // other way (SPT.Server, dotnet test, any Windows build) falls through to it.
                var mainProgram = NativeLibrary.GetMainProgramHandle();
                if (NativeLibrary.TryGetExport(mainProgram, "spt_native_abi_version", out _))
                {
                    return mainProgram;
                }

                var fileName =
                    OperatingSystem.IsWindows() ? "spt_native.dll"
                    : OperatingSystem.IsMacOS() ? "libspt_native.dylib"
                    : "libspt_native.so";
                NativeLibrary.TryLoad(Path.Combine(AppContext.BaseDirectory, fileName), out var handle);
                return handle;
            }
        );
    }

    [LibraryImport(LibraryName, EntryPoint = "spt_native_abi_version")]
    internal static partial uint AbiVersion();

    [LibraryImport(LibraryName, EntryPoint = "spt_verify_database")]
    internal static partial int VerifyDatabase(byte* dirUtf8, nuint dirLen, byte** outPtr, nuint* outLen);

    [LibraryImport(LibraryName, EntryPoint = "spt_generate_static_containers")]
    internal static partial int GenerateStaticContainers(byte* requestUtf8, nuint requestLen, byte** outPtr, nuint* outLen);

    [LibraryImport(LibraryName, EntryPoint = "spt_generate_dynamic_loot")]
    internal static partial int GenerateDynamicLoot(byte* requestUtf8, nuint requestLen, byte** outPtr, nuint* outLen);

    [LibraryImport(LibraryName, EntryPoint = "spt_create_random_loot")]
    internal static partial int CreateRandomLoot(byte* requestUtf8, nuint requestLen, byte** outPtr, nuint* outLen);

    [LibraryImport(LibraryName, EntryPoint = "spt_create_forced_loot")]
    internal static partial int CreateForcedLoot(byte* requestUtf8, nuint requestLen, byte** outPtr, nuint* outLen);

    [LibraryImport(LibraryName, EntryPoint = "spt_get_sealed_weapon_case_loot")]
    internal static partial int GetSealedWeaponCaseLoot(byte* requestUtf8, nuint requestLen, byte** outPtr, nuint* outLen);

    [LibraryImport(LibraryName, EntryPoint = "spt_get_random_loot_container_loot")]
    internal static partial int GetRandomLootContainerLoot(byte* requestUtf8, nuint requestLen, byte** outPtr, nuint* outLen);

    [LibraryImport(LibraryName, EntryPoint = "spt_generate_bot_inventory")]
    internal static partial int GenerateBotInventory(byte* requestUtf8, nuint requestLen, byte** outPtr, nuint* outLen);

    [LibraryImport(LibraryName, EntryPoint = "spt_generate_bot_inventory_batch")]
    internal static partial int GenerateBotInventoryBatch(byte* requestUtf8, nuint requestLen, byte** outPtr, nuint* outLen);

    [LibraryImport(LibraryName, EntryPoint = "spt_generate_dynamic_offers")]
    internal static partial int GenerateDynamicOffers(byte* requestUtf8, nuint requestLen, byte** outPtr, nuint* outLen);

    [LibraryImport(LibraryName, EntryPoint = "spt_generate_repeatable_quest")]
    internal static partial int GenerateRepeatableQuest(byte* requestUtf8, nuint requestLen, byte** outPtr, nuint* outLen);

    [LibraryImport(LibraryName, EntryPoint = "spt_generate_scav_case_rewards")]
    internal static partial int GenerateScavCaseRewards(byte* requestUtf8, nuint requestLen, byte** outPtr, nuint* outLen);

    [LibraryImport(LibraryName, EntryPoint = "spt_get_raid_adjustments")]
    internal static partial int GetRaidAdjustments(byte* requestUtf8, nuint requestLen, byte** outPtr, nuint* outLen);

    [LibraryImport(LibraryName, EntryPoint = "spt_make_adjustments_to_map")]
    internal static partial int MakeAdjustmentsToMap(byte* requestUtf8, nuint requestLen, byte** outPtr, nuint* outLen);

    [LibraryImport(LibraryName, EntryPoint = "spt_adjust_bot_hostility_settings")]
    internal static partial int AdjustBotHostilitySettings(byte* requestUtf8, nuint requestLen, byte** outPtr, nuint* outLen);

    [LibraryImport(LibraryName, EntryPoint = "spt_adjust_extracts")]
    internal static partial int AdjustExtracts(byte* requestUtf8, nuint requestLen, byte** outPtr, nuint* outLen);

    [LibraryImport(LibraryName, EntryPoint = "spt_apply_pmc_wave_changes")]
    internal static partial int ApplyPmcWaveChanges(byte* requestUtf8, nuint requestLen, byte** outPtr, nuint* outLen);

    [LibraryImport(LibraryName, EntryPoint = "spt_build_item_base_class_cache")]
    internal static partial int BuildItemBaseClassCache(byte* requestUtf8, nuint requestLen, byte** outPtr, nuint* outLen);

    [LibraryImport(LibraryName, EntryPoint = "spt_build_ragfair_linked_item_table")]
    internal static partial int BuildRagfairLinkedItemTable(byte* requestUtf8, nuint requestLen, byte** outPtr, nuint* outLen);

    [LibraryImport(LibraryName, EntryPoint = "spt_db_publish")]
    internal static partial int DbPublish(byte* requestUtf8, nuint requestLen, byte** outPtr, nuint* outLen);

    [LibraryImport(LibraryName, EntryPoint = "spt_db_load")]
    internal static partial int DbLoad(byte* requestUtf8, nuint requestLen, byte** outPtr, nuint* outLen);

    [LibraryImport(LibraryName, EntryPoint = "spt_db_resident_digest")]
    internal static partial int DbResidentDigest(byte** outPtr, nuint* outLen);

    [LibraryImport(LibraryName, EntryPoint = "spt_profile_list")]
    internal static partial int ProfileList(byte* requestUtf8, nuint requestLen, byte** outPtr, nuint* outLen);

    [LibraryImport(LibraryName, EntryPoint = "spt_profile_load")]
    internal static partial int ProfileLoad(byte* requestUtf8, nuint requestLen, byte** outPtr, nuint* outLen);

    [LibraryImport(LibraryName, EntryPoint = "spt_profile_save")]
    internal static partial int ProfileSave(byte* requestUtf8, nuint requestLen, byte** outPtr, nuint* outLen);

    [LibraryImport(LibraryName, EntryPoint = "spt_profile_delete")]
    internal static partial int ProfileDelete(byte* requestUtf8, nuint requestLen, byte** outPtr, nuint* outLen);

    [LibraryImport(LibraryName, EntryPoint = "spt_locales_set")]
    internal static partial int LocalesSet(byte* jsonUtf8, nuint jsonLen, byte** outPtr, nuint* outLen);

    [LibraryImport(LibraryName, EntryPoint = "spt_buf_free")]
    internal static partial void BufFree(byte* ptr, nuint len);
}
