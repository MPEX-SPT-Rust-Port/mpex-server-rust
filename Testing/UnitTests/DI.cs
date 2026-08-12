using System.Reflection;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging;
using NUnit.Framework;
using SPTarkov.Common.Models.Logging;
using SPTarkov.DI;
using SPTarkov.Server;
using SPTarkov.Server.Core.DI;
using SPTarkov.Server.Core.Loaders;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Launcher;
using SPTarkov.Server.Core.Models.Spt.Mod;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Services.Hosted;
using SPTarkov.Server.Extensions;
using SPTarkov.Server.Helpers;
using UnitTests.Mock;

namespace UnitTests;

[TestFixture]
public class DI
{
    private static IServiceProvider _serviceProvider = default!;

    private static DI? _instance;

    private DI()
    {
        ConfigureServices();
    }

    public static DI GetInstance()
    {
        return _instance ??= new DI();
    }

    private static DatabaseTables SetupDB(IReadOnlyDictionary<Type, BaseConfig> configuration, LocaleTable locales, ILogger logger)
    {
        var services = new ServiceCollection();

        services.AddSingleton(locales);
        foreach (var configEntry in configuration)
        {
            services.AddSingleton(configEntry.Key, configEntry.Value);
        }
        services.AddSingleton(logger);
        services.AddSingleton(typeof(ILogger<>), typeof(MockLogger<>));
        services.AddSingleton(typeof(ISptLogger<>), typeof(MockLogger<>));

        var diHandler = new DependencyInjectionHandler(services);
        diHandler.AddInjectableTypesFromAssembly(typeof(Program).Assembly);
        diHandler.AddInjectableTypesFromAssembly(typeof(SPTStartupHostedService).Assembly);
        diHandler.InjectAll();
        services.AddSingleton<DatabaseImporter>();

        var serviceProvider = services.BuildServiceProvider();
        var dbImporter = serviceProvider.GetRequiredService<DatabaseImporter>();
        var tables = dbImporter.LoadDatabaseAsync(false).GetAwaiter().GetResult();

        return tables is null ? throw new InvalidOperationException("Tables aren't loaded lol") : tables;
    }

    private void ConfigureServices()
    {
        if (_serviceProvider != null)
        {
            return;
        }

        _serviceProvider = BuildIsolatedProvider();
    }

    /// <summary>
    /// Builds a fully loaded provider (config, locales, database, all Core injectables, IOnLoad run),
    /// optionally scanning extra mod assemblies. Callers own disposal. Used by the shared singleton
    /// above and by fixtures that need an isolated container mod registrations can't leak out of.
    /// </summary>
    internal static IServiceProvider BuildIsolatedProvider(params Assembly[] modAssemblies)
    {
        var mockLogger = new MockLogger<DI>();
        var configuration = ConfigLoader.Initialize(mockLogger).GetAwaiter().GetResult();

        var services = new ServiceCollection();
        services.AddSingleton(mockLogger);
        services.AddSingleton(typeof(ILogger<>), typeof(MockLogger<>));
        services.AddSingleton(typeof(ISptLogger<>), typeof(MockLogger<>));
        services.AddHttpContextAccessor();
        services.AddHttpClient();
        services.AddSingleton(new ClientEnumDefinitions());

        var locales = ProgramHelpers.CreateEarlyLocaleTable() ?? throw new InvalidOperationException("Locales aren't loaded lmao");
        var db = SetupDB(configuration, locales, mockLogger);
        services.AddSingleton(db.Bots);
        services.AddSingleton(db.Hideout);
        services.AddSingleton(db.Locales);
        services.AddSingleton(db.Locations);
        services.AddSingleton(db.Match);
        services.AddSingleton(db.Templates);
        services.AddSingleton(db.Traders);
        services.AddSingleton(db.Globals);
        services.AddSingleton(db.Server);
        services.AddSingleton(db.Settings);

        foreach (var configEntry in configuration)
        {
            services.AddSingleton(configEntry.Key, configEntry.Value);
        }

        var diHandler = new DependencyInjectionHandler(services);

        diHandler.AddInjectableTypesFromTypeAssembly(typeof(SPTStartupHostedService));
        foreach (var modAssembly in modAssemblies)
        {
            diHandler.AddInjectableTypesFromAssembly(modAssembly);
        }

        diHandler.InjectAll();

        services.AddModDIConstructorsAsync(modAssemblies).GetAwaiter().GetResult();

        services.AddSingleton<IReadOnlyList<SptMod>>(_ => []);
        services.AddSingleton<IReadOnlyList<ModPage>>(_ => []);

        var serviceProvider = services.BuildServiceProvider();

        var cancellationTokenSource = new CancellationTokenSource();

        foreach (var onLoad in serviceProvider.GetServices<IOnLoad>())
        {
            onLoad.OnLoadAsync(cancellationTokenSource.Token).Wait();
        }

        return serviceProvider;
    }

    public T GetService<T>()
        where T : notnull
    {
        return _serviceProvider.GetRequiredService<T>();
    }
}
