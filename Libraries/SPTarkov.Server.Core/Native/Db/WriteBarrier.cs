using SPTarkov.Server.Core.Services.Server;

namespace SPTarkov.Server.Core.Native.Db;

/// <summary>
/// The one call target of the Ceciler-injected write barriers (Patches/Ceciler.WriteBarriers).
/// Model-type property setters have no path to the DI container, so the barrier calls this static
/// seam and it forwards to the process-global stamp.
///
/// This type exists to be rewritten: <see cref="Installed"/> returns false in source and the patch
/// replaces its body with `return true`, the way Utils/Reference/StaticReferences exists to serve
/// the ExtensionData patch. Do not "simplify" it to a constant.
/// </summary>
internal static class WriteBarrier
{
    [ThreadStatic]
    private static bool _suppressed;

    /// <summary>
    /// True only in a build whose SPTarkov.Server.Core.dll went through Ceciler (Release or
    /// publish). A Debug build has no barriers at all, so nothing may trust the resident DB with
    /// mods loaded there - see ResidentDbDispatch.Eligible.
    /// </summary>
    internal static bool Installed
    {
        // Ceciler.WriteBarriers rewrites this body to `ldc.i4.1; ret`.
        get { return false; }
    }

    internal static void Bump()
    {
        if (_suppressed)
        {
            return;
        }

        DatabaseMutationStamp.BumpGlobal();
    }

    /// <summary>
    /// Silences barriers on the calling thread. DbPublisher holds one of these across the whole
    /// projection: building the publish payload forces LazyLoad materialisation and HandbookHelper
    /// hydration, both of which write into the tables being serialized. Without this, every publish
    /// dirties the stamp it just read and EnsureCurrent republishes on every subsequent call,
    /// forever.
    /// </summary>
    internal static SuppressScope Suppress()
    {
        return new SuppressScope();
    }

    internal readonly struct SuppressScope : IDisposable
    {
        private readonly bool _previous;

        // C# requires a parameterless struct constructor to be public (CS8958); the struct itself
        // is internal, so this widens nothing.
        public SuppressScope()
        {
            _previous = _suppressed;
            _suppressed = true;
        }

        public void Dispose()
        {
            _suppressed = _previous;
        }
    }
}
