using System.Buffers.Binary;
using System.Numerics;
using System.Security.Cryptography;

namespace SPTarkov.Server.Core.Utils;

/// <summary>
///     The randomness primitives behind <see cref="RandomUtil"/> and
///     <see cref="Collections.ProbabilityObjectArray{K,V}"/>. Production uses
///     <see cref="CryptoRandomSource"/>, which calls exactly the statics the callers used to call.
///     Tests swap in a <see cref="SeededRandomSource"/>, whose draws are bit-identical to the Rust
///     seeded mode in rust/spt-native/src/loot/random_util.rs — pinned by RandomSourceParityTests
///     and the Rust KAT twins.
/// </summary>
internal interface IRandomSource
{
    int GetInt32(int fromInclusive, int toExclusive);

    /// <summary>
    ///     Uniform [0, 1) from 48 random bits with 0 folded to 1 — the
    ///     <see cref="RandomUtil.GetSecureRandomNumber"/> shape.
    /// </summary>
    double NextDouble48();

    /// <summary>
    ///     Uniform [0, 1) from 53 random bits — the <c>Random.Shared.NextDouble()</c> shape.
    /// </summary>
    double NextDouble53();

    void Fill(Span<byte> buffer);
}

internal sealed class CryptoRandomSource : IRandomSource
{
    internal static readonly CryptoRandomSource Instance = new();

    public int GetInt32(int fromInclusive, int toExclusive)
    {
        return RandomNumberGenerator.GetInt32(fromInclusive, toExclusive);
    }

    public double NextDouble48()
    {
        Span<byte> buffer = stackalloc byte[8];
        RandomNumberGenerator.Fill(buffer);

        var value = BinaryPrimitives.ReadInt64BigEndian(buffer) & 0x0000_FFFF_FFFF_FFFF;
        if (value == 0L)
        {
            value = 1L;
        }

        return value / 281474976710656.0;
    }

    public double NextDouble53()
    {
        return Random.Shared.NextDouble();
    }

    public void Fill(Span<byte> buffer)
    {
        RandomNumberGenerator.Fill(buffer);
    }
}

/// <summary>
///     xoshiro256** with splitmix64 seed expansion — the parity twin of <c>xoshiro_from_u64</c> +
///     <c>next_u64</c> in rust/spt-native/src/loot/random_util.rs. Not thread-safe; test use only.
/// </summary>
internal sealed class Xoshiro256StarStar
{
    private ulong _s0;
    private ulong _s1;
    private ulong _s2;
    private ulong _s3;

    public Xoshiro256StarStar(ulong seed)
    {
        _s0 = SplitMix64(ref seed);
        _s1 = SplitMix64(ref seed);
        _s2 = SplitMix64(ref seed);
        _s3 = SplitMix64(ref seed);
    }

    public ulong NextUInt64()
    {
        var result = BitOperations.RotateLeft(_s1 * 5, 7) * 9;
        var t = _s1 << 17;

        _s2 ^= _s0;
        _s3 ^= _s1;
        _s1 ^= _s2;
        _s0 ^= _s3;
        _s2 ^= t;
        _s3 = BitOperations.RotateLeft(_s3, 45);

        return result;
    }

    private static ulong SplitMix64(ref ulong state)
    {
        state += 0x9E3779B97F4A7C15UL;
        var z = state;
        z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9UL;
        z = (z ^ (z >> 27)) * 0x94D049BB133111EBUL;
        return z ^ (z >> 31);
    }
}

internal sealed class SeededRandomSource(ulong seed) : IRandomSource
{
    private readonly Xoshiro256StarStar _rng = new(seed);

    public int GetInt32(int fromInclusive, int toExclusive)
    {
        if (toExclusive <= fromInclusive)
        {
            throw new ArgumentOutOfRangeException(nameof(fromInclusive));
        }

        var range = (ulong)((long)toExclusive - fromInclusive);
        return (int)(fromInclusive + (long)NextBelow(range));
    }

    public double NextDouble48()
    {
        var value = _rng.NextUInt64() & 0x0000_FFFF_FFFF_FFFFUL;
        if (value == 0UL)
        {
            value = 1UL;
        }

        return value / 281474976710656.0;
    }

    public double NextDouble53()
    {
        return (_rng.NextUInt64() >> 11) * (1.0 / 9007199254740992.0);
    }

    public void Fill(Span<byte> buffer)
    {
        Span<byte> chunk = stackalloc byte[8];
        var remaining = buffer;
        while (remaining.Length > 0)
        {
            BinaryPrimitives.WriteUInt64LittleEndian(chunk, _rng.NextUInt64());

            var take = Math.Min(8, remaining.Length);
            chunk[..take].CopyTo(remaining);
            remaining = remaining[take..];
        }
    }

    /// <summary>
    ///     Uniform [0, range) by bitmask rejection; parity twin of <c>next_below</c> in
    ///     rust/spt-native/src/loot/random_util.rs.
    /// </summary>
    private ulong NextBelow(ulong range)
    {
        if (range <= 1)
        {
            return 0;
        }

        var mask = ulong.MaxValue >> BitOperations.LeadingZeroCount(range - 1);
        while (true)
        {
            var value = _rng.NextUInt64() & mask;
            if (value < range)
            {
                return value;
            }
        }
    }
}

/// <summary>
///     The swappable source behind <see cref="Collections.ProbabilityObjectArray{K,V}"/>, which
///     cannot take a constructor dependency without breaking the frozen 4.1.2 surface. Test-only
///     swap; always restore <see cref="CryptoRandomSource.Instance"/> in a finally.
/// </summary>
internal static class ProbabilityRandomSource
{
    internal static IRandomSource Current = CryptoRandomSource.Instance;
}
