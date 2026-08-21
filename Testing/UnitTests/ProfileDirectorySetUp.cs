using NUnit.Framework;
using UnitTests.Tests.Servers;

namespace UnitTests;

/// <summary>
/// Assembly-scoped, because nothing narrower is early enough. Every namespace in this assembly is
/// under <c>UnitTests</c>, and NUnit runs a <see cref="SetUpFixtureAttribute"/>'s
/// <see cref="OneTimeSetUpAttribute"/> before any fixture in its namespace or below — so this runs
/// before the first fixture of the run, whichever it is.
///
/// That matters because <c>DI.GetInstance()</c> builds its provider on first touch and runs every
/// <c>IOnLoad</c>, which means the first fixture to ask for any service is the one that triggers
/// <c>SaveCallbacks.OnLoadAsync</c> → <c>SaveServer.LoadAsync</c>. <c>UnitTests.Tests.Generators.*</c>
/// sorts ahead of <c>UnitTests.Tests.Servers.*</c>, so a sweep living in
/// <see cref="SaveServerPersistenceTests"/>'s own <c>[OneTimeSetUp]</c> normally runs *after* the
/// load it is meant to precede. A zero-byte profile leaked past a previous run — the artefact
/// <c>EmptyProfileFileTakesTheRecoveryArm</c> deliberately creates — would then redden the whole
/// suite from inside DI construction, with an error pointing nowhere near the cause.
///
/// The sweep is the whole directory, not a list of ids. <c>SaveAsync()</c> writes every profile the
/// process-wide <c>SaveServer</c> holds, including ones other fixtures created under
/// <c>new MongoId()</c>, so a hard kill — CI timeout, Ctrl-C — leaks files no id list can name.
/// Worse, the next run's <c>_preexistingFiles</c> snapshot would then adopt them and protect them
/// from ever being cleaned. The test bin directory has no legitimate profiles in it, so there is
/// nothing here to preserve.
/// </summary>
[SetUpFixture]
public class ProfileDirectorySetUp
{
    private const string ProfileDir = "user/profiles";

    [OneTimeSetUp]
    public void CleanLeakedProfiles()
    {
        if (Directory.Exists(ProfileDir))
        {
            Directory.Delete(ProfileDir, true);
        }
    }
}
