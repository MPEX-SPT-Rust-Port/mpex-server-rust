using System.Reflection;
using SPTarkov.Common.Models.Logging;
using SPTarkov.DI.Annotations;
using SPTarkov.Reflection.Patching;
using SPTarkov.Server.Core.DI;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Services.Locales;
using SPTarkov.Server.Core.Utils;

namespace TestMod;

/// <summary>
/// Scenario classes for the server's ModCompatibilityTests. Each one exercises a distinct
/// mod-pipeline guarantee; none of them are used by the TestMod's own runtime behavior.
/// </summary>
[Injectable(TypePriority = OnLoadOrder.Watermark + 1)]
public class TestModWatermarkOverride(
    ISptLogger<Watermark> logger,
    ServerLocalisationService serverLocalisationService,
    WatermarkLocale watermarkLocale,
    CoreConfig coreConfig
) : Watermark(logger, serverLocalisationService, watermarkLocale, coreConfig);

[Injectable(InjectionType.Singleton)]
public class TestModOnUpdate : IOnUpdate
{
    public Task<bool> OnUpdateAsync(long secondsSinceLastRun, CancellationToken cancellationToken)
    {
        return Task.FromResult(true);
    }
}

[Injectable(TypePriority = OnLoadOrder.Routers)]
public class TestModStaticRouter(JsonUtil jsonUtil)
    : StaticRouter(
        jsonUtil,
        [new RouteAction("/testmod/ping", (url, info, sessionId, output, cancellationToken) => new ValueTask<object>("pong"))]
    );

public class TestModHarmonyPatchTarget
{
    public int GetValue()
    {
        return 1;
    }
}

public class TestModHarmonyPatch : AbstractPatch
{
    protected override MethodBase? GetTargetMethod()
    {
        return typeof(TestModHarmonyPatchTarget).GetMethod(nameof(TestModHarmonyPatchTarget.GetValue));
    }

    // Enable/Disable check Assembly.GetCallingAssembly() against the assembly that constructed
    // the patch, so the test project can't call them directly — these wrappers keep the call
    // inside the owning (mod) assembly, the same way a real mod enables its own patches.
    public void Activate()
    {
        Enable();
    }

    public void Deactivate()
    {
        Disable();
    }

    [PatchPostfix]
    private static void Postfix(ref int __result)
    {
        __result = 2;
    }
}
