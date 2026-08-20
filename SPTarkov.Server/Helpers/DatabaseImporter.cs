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

            var dataToImport = await ImportTablesAsync(shouldVerifyDatabase, cancellationToken);

            timer.Stop();

            logger.Info(serverLocalisationService.GetText("importing_database_finish"));
            logger.Debug($"Database import took {timer.ElapsedMilliseconds}ms");

            return dataToImport;
        }
        catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested)
        {
            logger.Warning("Database import was cancelled.");

            throw;
        }
    }

    private async Task<DatabaseTables> ImportTablesAsync(bool shouldVerifyDatabase, CancellationToken cancellationToken)
    {
        if (coreConfig.ForceLegacyDatabaseImport)
        {
            if (shouldVerifyDatabase)
            {
                await VerifyDatabaseAsync();
            }

            return await importerUtil.LoadRecursiveAsync<DatabaseTables>($"{SptDataPath}database/", cancellationToken: cancellationToken);
        }

        // Fused native load: one walk hashes (when verifying) and reads; the reflection walk below
        // materializes from the returned buffers and only touches disk for LazyLoad content.
        // ponytail: epoch 1 is installed here but DbPublisher still republishes on its first
        // EnsureCurrent; skipping that republish when the stamp never moved is deliberately not built.
        var load = await Task.Run(() => SptNative.DbLoad(SptDataPath, shouldVerifyDatabase), cancellationToken);

        if (shouldVerifyDatabase)
        {
            ThrowIfVerificationFailed(load.Verify);
        }

        return await importerUtil.LoadRecursiveAsync<DatabaseTables>($"{SptDataPath}database/", load.Files, cancellationToken);
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
