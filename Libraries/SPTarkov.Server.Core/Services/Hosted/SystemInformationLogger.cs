using SPTarkov.Common.Models.Logging;
using SPTarkov.DI.Annotations;
using SPTarkov.Server.Core.Utils;
using static SPTarkov.Server.Core.Extensions.StringExtensions;

namespace SPTarkov.Server.Core.Services.Hosted;

/// <summary>
/// Logs system information (OS, RAM, CPU etc) on startup, at Info so it's captured by the shipped
/// Info-level file logger. The info lines use this class's own logger name, letting sptLogger.json exclude
/// them from the console while keeping them in the file. Warnings are logged under the startup service's
/// name so they stay visible on the console.
/// </summary>
[Injectable]
public sealed class SystemInformationLogger(
    ISptLogger<SystemInformationLogger> logger,
    ISptLogger<SPTStartupHostedService> hostedServiceLogger
)
{
    public void LogSystemInformation()
    {
        var totalMemoryBytes = GC.GetGCMemoryInfo().TotalAvailableMemoryBytes;

        // Convert bytes to GB
        var totalMemoryGb = totalMemoryBytes / (1024.0 * 1024.0 * 1024.0);
        var pageFileGb = Environment.SystemPageSize / 1024.0;

        logger.Info($"OS: {Environment.OSVersion.Version} | {Environment.OSVersion.Platform}");
        logger.Info($"Pagefile: {pageFileGb:F2} GB");
        if (pageFileGb <= 0 && Environment.OSVersion.Platform == PlatformID.Win32NT)
        {
            hostedServiceLogger.Warning("Pagefile size is 0 GB, you may encounter out of memory errors when loading into raids");
        }
        logger.Info($"RAM: {totalMemoryGb:F2} GB");
        if (totalMemoryGb < 30)
        {
            hostedServiceLogger.Warning(
                $"Detected RAM ({totalMemoryGb:F2}GB) is smaller than recommended (32GB) you may experience crashes or reduced FPS on large maps"
            );
        }
        logger.Info($"Ran as admin: {Environment.IsPrivilegedProcess}");
        logger.Info($"CPU cores: {Environment.ProcessorCount}");
        logger.Info($"PATH: {(Environment.ProcessPath ?? "null returned").Encode(EncodeType.BASE64)}");
        logger.Info($"Server: {ProgramStatics.SPT_VERSION()}");

        if (ProgramStatics.BUILD_TIME() != 0)
        {
            logger.Info($"Date: {ProgramStatics.BUILD_TIME()}");
        }

        logger.Info($"Commit: {ProgramStatics.COMMIT()}");
    }
}
