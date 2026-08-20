using System.Diagnostics;
using SPTarkov.Common.Models.Logging;
using SPTarkov.Server.Core.Exceptions.Database;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Native;
using SPTarkov.Server.Core.Services.Locales;
using SPTarkov.Server.Core.Utils;

namespace SPTarkov.Server.Helpers;

public sealed class DatabaseImporter(
    ISptLogger<DatabaseImporter> logger,
    ServerLocalisationService serverLocalisationService,
    ImporterUtil importerUtil,
    CoreConfig coreConfig
)
{
    private const string SptDataPath = "./SPT_Data/";

    /// <summary>
    /// Read all json files in database folder and map into a json object
    /// </summary>
    /// <param name="shouldVerifyDatabase">if the database should be verified before deserialization</param>
    /// <param name="cancellationToken">
    /// The <see cref="CancellationToken"/> that can be used to cancel the database hydration operation.
    /// </param>
    /// <returns></returns>
    public async Task<DatabaseTables?> LoadDatabaseAsync(bool shouldVerifyDatabase, CancellationToken cancellationToken = default)
    {
        try
        {
            // Even builds that skip verification depend on spt_native, so probe it here rather than
            // letting a missing or stale library surface as a DllNotFoundException mid-request.
            SptNative.EnsureLoadable();

            // Generator diagnostics render natively now; bake the ServerLocale -> en fallback
            // chain into a flat table once, before anything can invoke a generator.
            SptNative.SetServerLocales(
                serverLocalisationService.GetLocaleKeys().ToDictionary(key => key, serverLocalisationService.GetLocalisedValue)
            );

            logger.Info(serverLocalisationService.GetText("importing_database"));
            Stopwatch timer = new();
            timer.Start();

            var (dataToImport, preloadedFiles) = await ImportTablesAsync(shouldVerifyDatabase, cancellationToken);

            timer.Stop();

            logger.Info(serverLocalisationService.GetText("importing_database_finish"));

            // The buffer count is the only signal the native arm is really feeding the walk: a key drift
            // between spt_db_load and ImporterUtil falls back to disk silently and only costs time.
            logger.Debug(
                $"Database import took {timer.ElapsedMilliseconds}ms{(preloadedFiles is null ? "" : $", {preloadedFiles} files preloaded by spt_native")}"
            );

            return dataToImport;
        }
        catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested)
        {
            logger.Warning("Database import was cancelled.");

            throw;
        }
    }

    /// <summary>
    /// The tables, plus how many file buffers the native load fed the walk (null on the legacy arm,
    /// which has no map).
    /// </summary>
    private async Task<(DatabaseTables Tables, int? PreloadedFiles)> ImportTablesAsync(
        bool shouldVerifyDatabase,
        CancellationToken cancellationToken
    )
    {
        if (coreConfig.ForceLegacyDatabaseImport)
        {
            if (shouldVerifyDatabase)
            {
                await VerifyDatabaseAsync();
            }

            var legacy = await importerUtil.LoadRecursiveAsync<DatabaseTables>(
                $"{SptDataPath}database/",
                cancellationToken: cancellationToken
            );

            return (legacy, null);
        }

        // Fused native load: one walk hashes (when verifying) and reads; the reflection walk below
        // materializes from the returned buffers and only touches disk for LazyLoad content.
        // ponytail: epoch 1 is installed here but DbPublisher still republishes on its first
        // EnsureCurrent; skipping that republish when the stamp never moved is deliberately not built.
        DbLoadResult load;

        try
        {
            load = await Task.Run(() => SptNative.DbLoad(SptDataPath, shouldVerifyDatabase), cancellationToken);
        }
        catch (InvalidOperationException ex)
        {
            // A parse defect anywhere in templates/, traders/, globals.json, locations/ or hideout/ fails
            // here against one assembled envelope, so the reported line/column belong to that envelope and
            // name no file. Point at the way out rather than at a coordinate nobody can look up.
            throw new InvalidOperationException(
                "The native database load failed. Any line/column below is an offset into the single ~60MB "
                    + "envelope spt_db_load assembles from database/templates, traders, globals.json, locations "
                    + "and hideout - not into any one file, so do not go looking for that line on disk. Comments "
                    + "in those files are the usual cause: the C# reader skips them, the native parser rejects "
                    + "them. Set \"forceLegacyDatabaseImport\": true in SPT_Data/configs/core.json (add the key; "
                    + "it is not shipped) to restore the pure-C# import, which names the offending file.",
                ex
            );
        }

        if (shouldVerifyDatabase)
        {
            ThrowIfVerificationFailed(load.Verify);
        }

        var tables = await importerUtil.LoadRecursiveAsync<DatabaseTables>($"{SptDataPath}database/", load.Files, cancellationToken);

        return (tables, load.Files.Count);
    }

    private async Task VerifyDatabaseAsync()
    {
        Stopwatch timer = new();
        timer.Start();

        var result = await SptNative.VerifyDatabaseAsync(SptDataPath);

        timer.Stop();
        logger.Debug($"Database verification of {result.Checked} files took {timer.ElapsedMilliseconds}ms");

        ThrowIfVerificationFailed(result);
    }

    /// <summary>
    /// Shared failure handling for both verification arms: log every mismatch, then fail on the first.
    /// </summary>
    /// <param name="result">The report, or null when a verifying load answered without one.</param>
    private void ThrowIfVerificationFailed(VerifyResult? result)
    {
        if (result is null)
        {
            throw new InvalidOperationException("spt_native ran a verifying database load but returned no verification report.");
        }

        if (result.Ok)
        {
            return;
        }

        foreach (var failure in result.Failures)
        {
            logger.Error(serverLocalisationService.GetText("validation_error_file", $"{failure.Path} ({failure.Reason})"));
        }

        var firstFailure = result.Failures.FirstOrDefault()?.Path ?? "unknown (spt_native reported a failure with no details)";

        throw new ValidationErrorException(serverLocalisationService.GetText("validation_error_file", firstFailure));
    }
}
