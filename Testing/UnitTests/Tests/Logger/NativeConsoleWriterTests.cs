using System.Text;
using NUnit.Framework;
using SPTarkov.Common.Logger;

namespace UnitTests.Tests.Logger;

[TestFixture]
public class NativeConsoleWriterTests
{
    private static (NativeConsoleWriter writer, StringWriter fallback, List<(byte[] Bytes, bool ToStdErr)> forwarded) Create(
        bool forwardSucceeds,
        bool toStdErr = false
    )
    {
        var fallback = new StringWriter();
        var forwarded = new List<(byte[] Bytes, bool ToStdErr)>();
        var writer = new NativeConsoleWriter(
            fallback,
            toStdErr,
            (bytes, stream) =>
            {
                if (forwardSucceeds)
                {
                    forwarded.Add((bytes, stream));
                }

                return forwardSucceeds;
            }
        );

        return (writer, fallback, forwarded);
    }

    [Test]
    public void ForwardsWholeStringsAsSingleMessagesAndSkipsTheFallback()
    {
        var (writer, fallback, forwarded) = Create(forwardSucceeds: true);

        writer.WriteLine("hello world");
        writer.Write("partial");
        writer.Write('!');

        Assert.That(forwarded, Has.Count.EqualTo(3));
        Assert.That(Encoding.UTF8.GetString(forwarded[0].Bytes), Is.EqualTo("hello world" + Environment.NewLine));
        Assert.That(Encoding.UTF8.GetString(forwarded[1].Bytes), Is.EqualTo("partial"));
        Assert.That(Encoding.UTF8.GetString(forwarded[2].Bytes), Is.EqualTo("!"));
        Assert.That(fallback.ToString(), Is.Empty);
    }

    [Test]
    public void SpanAndCharArrayOverloadsForwardAsSingleMessages()
    {
        var (writer, _, forwarded) = Create(forwardSucceeds: true);

        writer.Write("span content".AsSpan());
        writer.WriteLine("span line".AsSpan());
        writer.Write(['a', 'b', 'c'], 1, 2);

        Assert.That(forwarded, Has.Count.EqualTo(3));
        Assert.That(Encoding.UTF8.GetString(forwarded[0].Bytes), Is.EqualTo("span content"));
        Assert.That(Encoding.UTF8.GetString(forwarded[1].Bytes), Is.EqualTo("span line" + Environment.NewLine));
        Assert.That(Encoding.UTF8.GetString(forwarded[2].Bytes), Is.EqualTo("bc"));
    }

    [Test]
    public void FallsBackToTheOriginalWriterWhenTheForwardDeclines()
    {
        var (writer, fallback, _) = Create(forwardSucceeds: false);

        writer.WriteLine("lost pipeline");

        Assert.That(fallback.ToString(), Is.EqualTo("lost pipeline" + Environment.NewLine));
    }

    [Test]
    public void ForwardsTheStdErrFlagItWasConstructedWith()
    {
        // Hardcoding the flag at the TryWrite call site would satisfy every other test in this
        // fixture while silently routing Console.Error to stdout, so assert both directions.
        foreach (var toStdErr in new[] { false, true })
        {
            var (writer, _, forwarded) = Create(forwardSucceeds: true, toStdErr);

            writer.WriteLine("routed");

            Assert.That(forwarded.Single().ToStdErr, Is.EqualTo(toStdErr));
        }
    }

    [Test]
    public void InstallIsIdempotentByAppContextMarker()
    {
        var originalOut = Console.Out;
        var originalError = Console.Error;

        try
        {
            NativeConsoleWriter.Install();
            var installed = Console.Out;
            var installedError = Console.Error;

            // Assert the first Install actually replaced both streams before asserting the second
            // one did not: without this, an Install() that does nothing at all would pass the
            // idempotence check below just as happily as a correct one.
            Assert.That(installed, Is.Not.SameAs(originalOut));
            Assert.That(installedError, Is.Not.SameAs(originalError));

            NativeConsoleWriter.Install();

            Assert.That(Console.Out, Is.SameAs(installed));
            Assert.That(Console.Error, Is.SameAs(installedError));
        }
        finally
        {
            Console.SetOut(originalOut);
            Console.SetError(originalError);
        }
    }
}
