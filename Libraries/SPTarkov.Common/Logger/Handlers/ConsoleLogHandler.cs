using SPTarkov.Common.Models.Logging;

namespace SPTarkov.Common.Logger.Handlers;

internal sealed class ConsoleLogHandler : BaseLogHandler
{
    public override LoggerType LoggerType
    {
        get { return LoggerType.Console; }
    }

    public override void Log(SptLogMessage message, BaseSptLoggerReference reference)
    {
        Console.WriteLine(FormatMessage(message.Message, message, reference));
    }

    public override ValueTask DisposeAsync()
    {
        GC.SuppressFinalize(this);
        return ValueTask.CompletedTask;
    }
}
