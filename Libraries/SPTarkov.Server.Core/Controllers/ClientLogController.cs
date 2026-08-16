using Microsoft.Extensions.Logging;
using SPTarkov.Common.Models.Logging;
using SPTarkov.DI.Annotations;
using SPTarkov.Server.Core.Models.Spt.Logging;

namespace SPTarkov.Server.Core.Controllers;

[Injectable]
public class ClientLogController(ISptLogger<ClientLogController> logger)
{
    /// <summary>
    ///     Handle /singleplayer/log
    /// </summary>
    public void ClientLog(ClientLogRequest logRequest)
    {
        logger.Log(logRequest.Level ?? LogLevel.Information, $"[{logRequest.Source}] {logRequest.Message}");
    }
}
