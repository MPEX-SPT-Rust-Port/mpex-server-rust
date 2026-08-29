using NUnit.Framework;
using SPTarkov.Server.Core.Generators.Bot;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Native.Db;
using SPTarkov.Server.Core.Services.Bot;
using SPTarkov.Server.Core.Services.Server;
using SPTarkov.Server.Core.Utils;

namespace UnitTests.Tests.Generators;

/// <summary>
/// The player scav family's half of the flip's core promise (<see cref="BotResidentDbTests"/>
/// precedent): the same seeded generation sent once off the resident DB and once with the C#-built
/// views override must produce identical scavs, field for field. The Rust-side resident test compares
/// against a hand-written views fixture, so nothing there could fail on a bug in the real
/// <c>PlayerScavNativeRequestBuilder.BuildViewsOverride</c> projection - this drives the two arms
/// through <c>Generate</c> itself, which is the only place that projection's real call site runs.
///
/// Mutates the shared <see cref="PlayerScavConfig"/> singleton, the <see cref="RandomUtil"/> seam and
/// the <see cref="ProbabilityRandomSource"/> static, so it restores all of them and never runs in
/// parallel with other fixtures.
/// </summary>
[TestFixture]
[NonParallelizable]
public class PlayerScavResidentDbTests
{
    private const ulong Seed = 424242;

    private PlayerScavGenerator _playerScavGenerator = default!;
    private PlayerScavConfig _playerScavConfig = default!;
    private BotNameService _botNameService = default!;
    private RandomUtil _randomUtil = default!;
    private JsonUtil _jsonUtil = default!;
    private DbPublisher _publisher = default!;
    private DatabaseMutationStamp _stamp = default!;

    private MongoId _sessionId;
    private bool _originalKillSwitch;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _playerScavGenerator = di.GetService<PlayerScavGenerator>();
        _playerScavConfig = di.GetService<PlayerScavConfig>();
        _botNameService = di.GetService<BotNameService>();
        _randomUtil = di.GetService<RandomUtil>();
        // Constructing JsonUtil is what publishes JsonSerializerOptionsNoIndent
        _jsonUtil = di.GetService<JsonUtil>();
        _publisher = di.GetService<DbPublisher>();
        _stamp = di.GetService<DatabaseMutationStamp>();

        _sessionId = PlayerScavProfileFixture.Create();
        _originalKillSwitch = _playerScavConfig.DisableNativeRequestCache;
    }

    [OneTimeTearDown]
    public void OneTimeTearDown()
    {
        _playerScavConfig.DisableNativeRequestCache = _originalKillSwitch;
        _playerScavGenerator.NativeTestSeed = null;
        // leave the shared container fresher than we found it for whatever fixture runs next
        _stamp.Bump();
    }

    /// <summary>
    /// The kill switch is the one flag that reaches the override arm without leaving the native path
    /// (<c>PlayerScavPathDispatchTests.TheKillSwitchForcesTheViewsOverrideWithoutLeavingTheNativePath</c>),
    /// so it is what separates the two arms here. Both arms are arm-asserted through
    /// <c>LastSendIncludedViewsOverride</c>: without that, a fixture that silently sent the override
    /// twice would compare a thing against itself and pass forever.
    /// </summary>
    [Test]
    public void AResidentSendAndAnOverrideSendProduceIdenticalScavsFieldForField()
    {
        // Settle the publisher's remembered epoch first, so the resident arm is not paying for
        // another fixture's desync
        _publisher.EnsureCurrent();

        _playerScavConfig.DisableNativeRequestCache = false;
        var resident = Normalize(GenerateArm());
        Assert.That(
            _playerScavGenerator.LastSendIncludedViewsOverride,
            Is.False,
            "the resident arm sent the views override - the comparison below would be vacuous"
        );

        // The real BuildViewsOverride(request.LootPools) call site runs on this arm and only this one
        _playerScavConfig.DisableNativeRequestCache = true;
        try
        {
            var @override = Normalize(GenerateArm());
            Assert.That(_playerScavGenerator.LastSendIncludedViewsOverride, Is.True, "the kill switch did not force the views override");

            LootJsonAssert.AssertEqual(resident, @override, "pscav resident-vs-override", Seed);
        }
        finally
        {
            _playerScavConfig.DisableNativeRequestCache = false;
        }
    }

    /// <summary>
    /// One arm, under <see cref="PlayerScavParityTests.GenerateArm"/>'s seeding discipline. Both arms
    /// here run the same C#-side prelude - bot level, nickname, voice, health and skills are drawn
    /// from <see cref="RandomUtil"/> and <see cref="ProbabilityRandomSource"/> on every
    /// <c>Generate</c>, and <c>NativeTestSeed</c> seeds only the native request - so without the
    /// dual-seam seeding the two arms would differ on Info/Customization/Health/Skills and the
    /// comparison would fail for reasons that have nothing to do with the views override.
    /// </summary>
    private PmcData GenerateArm()
    {
        // Generation writes the scav back into the profile, so both arms have to start from an
        // identical one
        PlayerScavProfileFixture.Reseed(_sessionId);

        // "assault" is in botRolesThatMustHaveUniqueName, so the nickname draw is retried until it
        // finds a name the process-wide cache has not handed out yet - and the first arm puts its
        // name in that cache. Left alone, the second arm rejects the name the first one took and
        // redraws, which shifts every C#-side draw after it by two.
        _botNameService.ClearNameCache();

        // Restore-what-you-captured (BotParityTests precedent): these are process-wide statics
        // shared across NUnit fixtures - do not assume the previous value was Crypto.
        var previousRandomSource = _randomUtil.RandomSource;
        var previousProbabilitySource = ProbabilityRandomSource.Current;
        var seeded = new SeededRandomSource(Seed);
        _randomUtil.RandomSource = seeded;
        ProbabilityRandomSource.Current = seeded;
        _playerScavGenerator.NativeTestSeed = Seed;
        try
        {
            var scav = _playerScavGenerator.Generate(_sessionId);
            Assert.That(_playerScavGenerator.LastPathTaken, Is.EqualTo(LootGenerationPath.Native));

            return scav;
        }
        finally
        {
            _playerScavGenerator.NativeTestSeed = null;
            _randomUtil.RandomSource = previousRandomSource;
            ProbabilityRandomSource.Current = previousProbabilitySource;
        }
    }

    private string Normalize(PmcData scav)
    {
        return PlayerScavJson.Normalize(_jsonUtil.Serialize(scav)!);
    }
}
