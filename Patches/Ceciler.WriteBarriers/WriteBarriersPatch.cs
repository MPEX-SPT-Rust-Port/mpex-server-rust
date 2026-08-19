using Mono.Cecil;
using Mono.Cecil.Cil;

namespace Ceciler.WriteBarriers;

/// <summary>
/// Prepends a `call WriteBarrier::Bump()` to the property setters of every DB-only model type
/// reachable from the roots DbPublisher publishes, so a mod writing game data dirties the resident
/// DB without a hand-written Bump() call.
///
/// Scope is a reachability walk, deliberately not the namespace sweep the spec first described.
/// Namespaces are wrong in both directions: Eft.Common.Location and Models/Eft/Hideout/* are
/// reachable from published roots but sit outside Models/Spt/Tables and Models/Eft/Common/Tables,
/// while those same namespaces also define BotBase (which PmcData derives from) and Item - live
/// per-request objects whose setters fire thousands of times per raid. Barriering either turns one
/// inventory write into a full resident-DB republish on the next native call.
/// </summary>
public class WriteBarriersPatch : IPatcher
{
    /// <summary>
    /// The roots DbPublisher actually ships (DbPublisher.cs ctor). A barrier on a non-resident
    /// root can only buy a republish that changes nothing, so the walk starts here and nowhere else.
    /// </summary>
    private static readonly string[] _publishedRoots =
    [
        "SPTarkov.Server.Core.Models.Spt.Tables.TemplateTable",
        "SPTarkov.Server.Core.Models.Spt.Tables.TradersTable",
        "SPTarkov.Server.Core.Models.Spt.Tables.GlobalTable",
        "SPTarkov.Server.Core.Models.Spt.Tables.LocationTable",
        "SPTarkov.Server.Core.Models.Spt.Tables.HideoutTable",
    ];

    /// <summary>
    /// Reachable from a published root, but also used as live per-request state. Barriering these
    /// is a republish storm, not a freshness win. Each entry needs a reason.
    /// </summary>
    private static readonly string[] _denied =
    [
        // Trader assorts and presets hold List<Item>, but Item is also every profile inventory
        // entry and every generated loot item - 91 setters on the hottest object in the server.
        "SPTarkov.Server.Core.Models.Eft.Common.Tables.Item",
        // BotBase is PmcData's base type: 271 setters fired on every profile write.
        "SPTarkov.Server.Core.Models.Eft.Common.Tables.BotBase",
        // Per-profile quest progress, not the quest template.
        "SPTarkov.Server.Core.Models.Eft.Common.Tables.PmcDataRepeatableQuest",
    ];

    private const string BarrierTypeName = "SPTarkov.Server.Core.Native.Db.WriteBarrier";

    public void Patch(AssemblyDefinition assembly)
    {
        var module = assembly.MainModule;

        var barrierType =
            module.GetType(BarrierTypeName)
            ?? throw new InvalidOperationException($"{BarrierTypeName} not found - the write-barrier seam must ship in this assembly");
        var bump =
            barrierType.Methods.FirstOrDefault(m => m.Name == "Bump")
            ?? throw new InvalidOperationException($"{BarrierTypeName}.Bump() not found");
        var installed =
            barrierType.Methods.FirstOrDefault(m => m.Name == "get_Installed")
            ?? throw new InvalidOperationException($"{BarrierTypeName}.Installed getter not found");

        var barriered = 0;
        foreach (var type in CollectReachableTypes(module))
        {
            foreach (var property in type.Properties)
            {
                if (ShouldBarrier(property))
                {
                    InjectBump(property.SetMethod, bump);
                    barriered++;
                }
            }
        }

        if (barriered == 0)
        {
            throw new InvalidOperationException("write-barrier patch matched no setters - the root or denylist names have drifted");
        }

        MarkInstalled(installed);
        Console.WriteLine($"WriteBarriers: {barriered} setters instrumented");

        assembly.Write(new WriterParameters { WriteSymbols = true });
    }

    /// <summary>
    /// Breadth-first over property types from the published roots, staying inside this assembly's
    /// Models namespace. Element types of generic collections are followed (List&lt;Preset&gt;
    /// reaches Preset), arrays are followed through their element type.
    /// </summary>
    private static List<TypeDefinition> CollectReachableTypes(ModuleDefinition module)
    {
        var seen = new HashSet<string>();
        var result = new List<TypeDefinition>();
        var queue = new Queue<TypeDefinition>();

        foreach (var rootName in _publishedRoots)
        {
            var root = module.GetType(rootName) ?? throw new InvalidOperationException($"published root {rootName} not found");
            if (seen.Add(root.FullName))
            {
                queue.Enqueue(root);
                result.Add(root);
            }
        }

        while (queue.Count > 0)
        {
            var current = queue.Dequeue();
            foreach (var property in current.Properties)
            {
                foreach (var candidate in Unwrap(property.PropertyType))
                {
                    if (!IsBarrierCandidate(candidate, out var definition) || !seen.Add(definition.FullName))
                    {
                        continue;
                    }

                    result.Add(definition);
                    queue.Enqueue(definition);
                }
            }
        }

        return result;
    }

    /// <summary>
    /// A type reference plus, for a generic instance or array, its argument/element types -
    /// Dictionary&lt;MongoId, TemplateItem&gt; yields MongoId and TemplateItem.
    /// </summary>
    private static IEnumerable<TypeReference> Unwrap(TypeReference reference)
    {
        yield return reference;

        if (reference is GenericInstanceType generic)
        {
            foreach (var argument in generic.GenericArguments)
            {
                foreach (var nested in Unwrap(argument))
                {
                    yield return nested;
                }
            }
        }

        if (reference is ArrayType array)
        {
            foreach (var nested in Unwrap(array.ElementType))
            {
                yield return nested;
            }
        }
    }

    private static bool IsBarrierCandidate(TypeReference reference, out TypeDefinition definition)
    {
        definition = null!;

        // Only types defined in the assembly being rewritten - Resolve() on a BCL reference would
        // reach a different module we must not touch.
        if (reference.Scope != reference.Module.Assembly.Name && reference.Scope is not ModuleDefinition)
        {
            return false;
        }

        var resolved = reference.Resolve();
        if (resolved is null || resolved.Module != reference.Module)
        {
            return false;
        }

        if (!resolved.FullName.StartsWith("SPTarkov.Server.Core.Models.", StringComparison.Ordinal))
        {
            return false;
        }

        if (resolved.IsInterface || resolved.IsEnum || resolved.HasGenericParameters || _denied.Contains(resolved.FullName))
        {
            return false;
        }

        definition = resolved;

        return true;
    }

    private static bool ShouldBarrier(PropertyDefinition property)
    {
        var setter = property.SetMethod;
        if (setter is null || setter.IsStatic || !setter.HasBody || setter.Body.Instructions.Count == 0)
        {
            return false;
        }

        // init accessors only ever run during construction and `with`, so a barrier there fires
        // once per deserialized object and conveys no freshness - pure startup cost.
        if (IsInitOnly(setter))
        {
            return false;
        }

        // The ExtensionData property is itself Ceciler-injected (by the sibling patch). This patch
        // runs first so it should not exist yet; skip by name in case the Exec order ever flips.
        if (property.Name == "ExtensionData")
        {
            return false;
        }

        // Explicit idempotency guard. The build's obj->bin copy normally restores pristine IL
        // before each rewrite, but that is a side effect of SkipUnchangedFiles, not a contract.
        if (
            setter.Body.Instructions[0].OpCode == OpCodes.Call
            && setter.Body.Instructions[0].Operand is MethodReference existing
            && existing.Name == "Bump"
        )
        {
            return false;
        }

        // Inserting before instruction 0 does not retarget branches or handler boundaries that
        // point at it. Auto-property setters have neither; refuse anything that might.
        if (setter.Body.HasExceptionHandlers)
        {
            return false;
        }

        var first = setter.Body.Instructions[0];

        return !setter.Body.Instructions.Any(instruction => ReferenceEquals(instruction.Operand, first));
    }

    private static bool IsInitOnly(MethodDefinition setter)
    {
        return setter.ReturnType is RequiredModifierType modifier
            && modifier.ModifierType.FullName == "System.Runtime.CompilerServices.IsExternalInit";
    }

    private static void InjectBump(MethodDefinition setter, MethodReference bump)
    {
        var il = setter.Body.GetILProcessor();
        il.InsertBefore(setter.Body.Instructions[0], il.Create(OpCodes.Call, bump));
    }

    private static void MarkInstalled(MethodDefinition installed)
    {
        var il = installed.Body.GetILProcessor();
        installed.Body.Instructions.Clear();
        il.Append(il.Create(OpCodes.Ldc_I4_1));
        il.Append(il.Create(OpCodes.Ret));
    }

    public string Name
    {
        get { return "WriteBarriers"; }
    }
}
