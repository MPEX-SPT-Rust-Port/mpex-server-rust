using System.Diagnostics;
using SPTarkov.Common.Models.Logging;
using SPTarkov.Server.Core.Exceptions.Database;
using SPTarkov.Server.Core.Native;
using SPTarkov.Server.Core.Services.Locales;
using SPTarkov.Server.Core.Utils;

namespace SPTarkov.Server.Helpers;

public sealed class DatabaseImporter(
    ISptLogger<DatabaseImporter> logger,
    ServerLocalisationService serverLocalisationService,
    ImporterUtil importerUtil
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

            if (shouldVerifyDatabase)
            {
                await VerifyDatabaseAsync();
            }

            logger.Info(serverLocalisationService.GetText("importing_database"));
            Stopwatch timer = new();
            timer.Start();

            var dataToImport = await importerUtil.LoadRecursiveAsync<DatabaseTables>(
                $"{SptDataPath}database/",
                cancellationToken: cancellationToken
            );

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

    private async Task VerifyDatabaseAsync()
    {
        Stopwatch timer = new();
        timer.Start();

        var result = await SptNative.VerifyDatabaseAsync(SptDataPath);

        timer.Stop();
        logger.Debug($"Database verification of {result.Checked} files took {timer.ElapsedMilliseconds}ms");

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
