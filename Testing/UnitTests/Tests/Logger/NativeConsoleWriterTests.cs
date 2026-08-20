using System.Text;
using NUnit.Framework;
using SPTarkov.Common.Logger;

namespace UnitTests.Tests.Logger;

[TestFixture]
public class NativeConsoleWriterTests
{
    private static (NativeConsoleWriter writer, StringWriter fallback, List<byte[]> forwarded) Create(bool forwardSucceeds)
    {
        var fallback = new StringWriter();
        var forwarded = new List<byte[]>();
        var writer = new NativeConsoleWriter(
            fallback,
            toStdErr: false,
            (bytes, _) =>
            {
                if (forwardSucceeds)
                {
                    forwarded.Add(bytes);
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
        Assert.That(Encoding.UTF8.GetString(forwarded[0]), Is.EqualTo("hello world" + Environment.NewLine));
        Assert.That(Encoding.UTF8.GetString(forwarded[1]), Is.EqualTo("partial"));
        Assert.That(Encoding.UTF8.GetString(forwarded[2]), Is.EqualTo("!"));
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
        Assert.That(Encoding.UTF8.GetString(forwarded[0]), Is.EqualTo("span content"));
        Assert.That(Encoding.UTF8.GetString(forwarded[1]), Is.EqualTo("span line" + Environment.NewLine));
        Assert.That(Encoding.UTF8.GetString(forwarded[2]), Is.EqualTo("bc"));
    }

    [Test]
    public void FallsBackToTheOriginalWriterWhenTheForwardDeclines()
    {
        var (writer, fallback, _) = Create(forwardSucceeds: false);

        writer.WriteLine("lost pipeline");

        Assert.That(fallback.ToString(), Is.EqualTo("lost pipeline" + Environment.NewLine));
    }

    [Test]
    public void InstallIsIdempotentByTypeName()
    {
        var originalOut = Console.Out;
        var originalError = Console.Error;

        try
        {
            NativeConsoleWriter.Install();
            var installed = Console.Out;
            NativeConsoleWriter.Install();

            Assert.That(Console.Out, Is.SameAs(installed));
        }
        finally
        {
            Console.SetOut(originalOut);
            Console.SetError(originalError);
        }
    }
}
