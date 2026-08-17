using System.Runtime.InteropServices;
using System.Text.Json;
using SPTarkov.Common.Logger;
using SPTarkov.Common.Models.Logging;
using SPTarkov.Common.Native;

namespace SPTarkov.Common.Extensions;

public static class SptLoggerExtensions
{
    private const string ConfigurationPath = "./sptLogger.json";
    private const string ConfigurationPathDev = "./sptLogger.Development.json";

    private static SptLoggerConfiguration LoadConfig(string configPath)
    {
        if (File.Exists(configPath))
        {
            using (FileStream fs = new(configPath, FileMode.Open, FileAccess.Read))
            {
                return JsonSerializer.Deserialize<SptLoggerConfiguration>(fs)
                    ?? throw new InvalidDataException($"Could not read SPTLogger config file {configPath}");
            }
        }
        else
        {
            throw new Exception($"Unable to find SPTLogger file '{configPath}'");
        }
    }

    /// <summary>
    /// Hands the raw sptLogger.json bytes to the native pipeline. Usually runs once per process,
    /// but a prepatched server's nested Program.Main inits again — the native side ref-counts, so
    /// that container's dispose does not take the outer host's logging down. Never throws: per the
    /// port's contract a broken native library or config gets one stderr notice and logging stays
    /// off.
    /// </summary>
    private static void InitNativeLogger(string configPath)
    {
        var configBytes = File.ReadAllBytes(configPath);
        nint messagePtr = 0;
        nuint messageLen = 0;

        try
        {
            var status = NativeMethods.LoggerInit(configBytes, (nuint)configBytes.Length, out messagePtr, out messageLen);

            if (status != 0)
            {
                var message = messagePtr == 0 ? $"internal status {status}" : Marshal.PtrToStringUTF8(messagePtr, checked((int)messageLen));

                Console.Error.WriteLine(
                    $"Failed to initialise the native log pipeline from '{configPath}': {message}. Logging is disabled."
                );
            }
        }
        catch (Exception exception) when (exception is DllNotFoundException or EntryPointNotFoundException)
        {
            Console.Error.WriteLine(
                $"Failed to load spt_native for logging: {exception.Message}. "
                    + "Rebuild the native library (dotnet build runs cargo automatically). Logging is disabled."
            );

            // The library is unloadable, so the buffer-free below would throw the same way.
            return;
        }

        if (messagePtr != 0)
        {
            NativeMethods.BufFree(messagePtr, messageLen);
        }
    }

    public static IHostBuilder UseSptLoggerWithoutProvider(this IHostBuilder builder, IServiceProvider earlyLoggerServiceProvider)
    {
        ArgumentNullException.ThrowIfNull(builder);

        builder.ConfigureServices(
            (_, collection) =>
            {
                collection.AddSptLoggerWithoutProvider(earlyLoggerServiceProvider);
            }
        );

        return builder;
    }

    public static IServiceCollection AddSptLogger(this IServiceCollection collection, bool isDevelop = false)
    {
        ArgumentNullException.ThrowIfNull(collection);

        if (isDevelop)
        {
            collection.AddSingleton(LoadConfig(ConfigurationPathDev));
            InitNativeLogger(ConfigurationPathDev);
        }
        else
        {
            collection.AddSingleton(LoadConfig(ConfigurationPath));
            InitNativeLogger(ConfigurationPath);
        }

        collection.AddSingleton<SPTLoggerDispatcher>();
        collection.AddSingleton<SptLoggerProvider>();
        collection.AddSingleton<ILoggerProvider>(sp => sp.GetRequiredService<SptLoggerProvider>());
        collection.AddSingleton<ILoggerFactory>(sp => sp.GetRequiredService<SptLoggerProvider>());

        collection.AddTransient(typeof(SptLogger<>));
        collection.AddTransient(typeof(ISptLogger<>), typeof(SptLogger<>));

        return collection;
    }

    public static IServiceCollection AddSptLoggerWithoutProvider(
        this IServiceCollection collection,
        IServiceProvider earlyLoggerServiceProvider
    )
    {
        collection.AddSingleton(earlyLoggerServiceProvider.GetRequiredService<SptLoggerConfiguration>());
        collection.AddSingleton(earlyLoggerServiceProvider.GetRequiredService<SPTLoggerDispatcher>());
        collection.AddTransient(typeof(SptLogger<>));
        collection.AddTransient(typeof(ISptLogger<>), typeof(SptLogger<>));
        return collection;
    }
}
