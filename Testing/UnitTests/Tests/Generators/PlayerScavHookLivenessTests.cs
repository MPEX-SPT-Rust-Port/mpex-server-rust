using System.Reflection;
using HarmonyLib;
using NUnit.Framework;
using SPTarkov.Server.Core.Generators.Bot;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Models.Common;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Pins the mod hook contract for player scav generation: a Harmony patch on any of the nine frozen
/// members must route generation back to the retained C# implementation, which is the only body
/// those patches can hook. Six are <see cref="PlayerScavGenerator"/>'s own protected members; the
/// other three belong to the bot shell the native arm reimplements or bypasses -
/// <c>BotGenerator.GeneratePlayerScav</c>, <c>BotGenerator.GenerateBot</c> and
/// <c>BotInventoryGenerator.GenerateInventory</c>.
///
/// The seven members that run C#-side on both arms are deliberately outside the set, along with
/// <c>Generate</c> itself - a patch there wraps whichever path runs
/// (<see cref="PlayerScavPathDispatchTests"/> covers that rule).
///
/// Harmony patches are process-wide, so every patch is removed in a finally and the fixture never
/// runs in parallel with others.
/// </summary>
[TestFixture]
[NonParallelizable]
public class PlayerScavHookLivenessTests
{
    /// <summary>
    /// The members that run C#-side on both arms, so no patch on them can be a reason to decline
    /// the native path.
    /// </summary>
    private static readonly string[] _bothArmsMembers =
    [
        nameof(PlayerScavGenerator.Generate),
        "AdjustItemWeights",
        "GetKarmaLimitValuesByKey",
        "GetScavStats",
        "GetScavLevel",
        "GetScavExperience",
        "SetScavCooldownTimer",
    ];

    private PlayerScavGenerator _playerScavGenerator = default!;

    private MongoId _sessionId;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        _playerScavGenerator = DI.GetInstance().GetService<PlayerScavGenerator>();
        _sessionId = PlayerScavProfileFixture.Create();
    }

    /// <summary>
    /// A live patch on any of the nine members the native arm replaces or bypasses routes
    /// generation back to C#, because those bodies are the only thing the patch can hook.
    /// </summary>
    [TestCaseSource(nameof(FrozenMembers))]
    public void AHarmonyPatchOnAFrozenMemberForcesTheLegacyPath(MethodBase member)
    {
        var harmony = new Harmony($"unit-tests.player-scav-hook-liveness.{member.DeclaringType!.Name}.{member.Name}");

        try
        {
            harmony.Patch(member, postfix: new HarmonyMethod(typeof(PlayerScavHookLivenessTests), nameof(PatchFired)));

            Assert.That(
                Harmony.GetPatchInfo(member)?.Postfixes.Any(patch => patch.owner == harmony.Id),
                Is.True,
                $"patch on {member.Name} was not registered"
            );

            PlayerScavProfileFixture.Reseed(_sessionId);
            var scav = _playerScavGenerator.Generate(_sessionId);

            Assert.That(
                _playerScavGenerator.LastPathTaken,
                Is.EqualTo(LootGenerationPath.Legacy),
                $"a patch on {member.DeclaringType.Name}.{member.Name} did not force the legacy path"
            );
            Assert.That(scav.Inventory!.Items, Is.Not.Empty, "the legacy path produced no inventory");
        }
        finally
        {
            harmony.UnpatchSelf();
        }
    }

    /// <summary>
    /// The frozen set is a hand-written list on the generator; this recomputes it off the
    /// generator's own surface, so a member added there without being added to the set - or
    /// deliberately excluded from it - fails loudly rather than silently going unhookable. The
    /// count catches the other rot: a name lookup that stops resolving is dropped by the list's
    /// <c>OfType</c> and would otherwise shrink the set unnoticed.
    /// </summary>
    [Test]
    public void TheFrozenSetIsTheGeneratorSurfaceMinusTheBothArmsMembersPlusTheThreeCrossTypeMembers()
    {
        var members =
            (List<MethodBase>)
                typeof(PlayerScavGenerator).GetField("_hookableMembers", BindingFlags.Static | BindingFlags.NonPublic)!.GetValue(null)!;

        Assert.That(members, Is.EquivalentTo(FrozenSurface()), "the frozen set is not the generator surface minus the both-arms members");
        Assert.That(members, Has.Count.EqualTo(9), "the frozen set is nine members");
    }

    private static IEnumerable<TestCaseData> FrozenMembers()
    {
        return FrozenSurface().Select(member => new TestCaseData(member).SetArgDisplayNames($"{member.DeclaringType!.Name}.{member.Name}"));
    }

    /// <summary>
    /// The nine members a mod can patch to take over part of player scav generation, recomputed
    /// independently of the generator's own list: its hookable surface minus the members both arms
    /// run C#-side, plus the three the bot shell owns. Constructors and property accessors are not
    /// hookable state here - <c>GetMethods</c> never returns the former and IsSpecialName drops the
    /// latter.
    /// </summary>
    private static List<MethodBase> FrozenSurface()
    {
        return
        [
            .. typeof(PlayerScavGenerator)
                .GetMethods(
                    BindingFlags.Instance | BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly
                )
                .Where(method => !method.IsSpecialName && !method.IsAbstract)
                .Where(method => method.IsPublic || method.IsFamily || method.IsFamilyOrAssembly)
                .Where(method => !_bothArmsMembers.Contains(method.Name)),
            Member(typeof(BotGenerator), nameof(BotGenerator.GeneratePlayerScav)),
            Member(typeof(BotGenerator), "GenerateBot"),
            Member(typeof(BotInventoryGenerator), nameof(BotInventoryGenerator.GenerateInventory)),
        ];
    }

    private static MethodBase Member(Type declaringType, string name)
    {
        return declaringType.GetMethod(
                name,
                BindingFlags.Instance | BindingFlags.NonPublic | BindingFlags.Public | BindingFlags.DeclaredOnly
            ) ?? throw new InvalidOperationException($"{declaringType.Name}.{name} is not declared any more");
    }

    private static void PatchFired() { }
}
