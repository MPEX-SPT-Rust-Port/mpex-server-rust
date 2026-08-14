using System.Buffers;
using System.Text.Json;
using MessagePack;
using NUnit.Framework;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Native.Ragfair;
using SPTarkov.Server.Core.Utils;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Pins the <c>encoding == 1</c> arm of the framed ragfair envelope against hand-built MessagePack
/// buffers. Nothing here touches the native library: the wire contract is string-keyed maps under
/// the same JSON names <c>rmp_serde::to_vec_named</c> emits, so a writer in the test is a faithful
/// stand-in for Rust and the reader can land before Rust ever sets the tag.
/// </summary>
[TestFixture]
public class MsgpackOfferReaderTests
{
    private const string OfferId = "5c0e2ff6d174af02a1659d4a";
    private const string RootId = "5c0e2ff6d174af02a1659d4b";
    private const string UserId = "5c0e2ff6d174af02a1659d4c";
    private const string ItemId = "5c0e2ff6d174af02a1659d4d";
    private const string TemplateId = "5449016a4bdc2d6f028b456f";
    private const string PresetId = "584148f2245977598f1ad387";

    [OneTimeSetUp]
    public void Initialize()
    {
        // Publishes the static JsonSerializerOptions the reader transcodes unknown values with
        DI.GetInstance().GetService<JsonUtil>();
    }

    [Test]
    public void AMinimalOfferMaterializesEveryKnownMember()
    {
        var payload = Build(
            (ref MessagePackWriter writer) =>
            {
                writer.WriteMapHeader(15);

                writer.Write("_id");
                writer.Write(OfferId);
                writer.Write("intId");
                writer.Write(7);
                writer.Write("user");
                WriteUser(ref writer);
                writer.Write("root");
                writer.Write(RootId);
                writer.Write("items");
                writer.WriteArrayHeader(1);
                writer.WriteMapHeader(3);
                writer.Write("_id");
                writer.Write(ItemId);
                writer.Write("_tpl");
                writer.Write(TemplateId);
                writer.Write("parentId");
                writer.WriteNil();
                writer.Write("itemsCost");
                writer.Write(12.5);
                writer.Write("requirements");
                writer.WriteArrayHeader(1);
                writer.WriteMapHeader(5);
                writer.Write("_tpl");
                writer.Write(TemplateId);
                writer.Write("count");
                writer.Write(1500.0);
                writer.Write("onlyFunctional");
                writer.Write(true);
                writer.Write("level");
                writer.Write(2);
                writer.Write("side");
                writer.Write(1);
                writer.Write("requirementsCost");
                writer.Write(1500.0);
                writer.Write("summaryCost");
                writer.Write(1500.0);
                writer.Write("startTime");
                writer.Write(1_700_000_000L);
                writer.Write("endTime");
                writer.Write(1_700_003_600L);
                writer.Write("loyaltyLevel");
                writer.Write(3);
                writer.Write("sellInOnePiece");
                writer.Write(true);
                writer.Write("locked");
                writer.Write(false);
                writer.Write("quantity");
                writer.Write(9);
            }
        );

        var offer = MsgpackOfferReader.ReadOffer(payload);

        Assert.Multiple(() =>
        {
            Assert.That(offer.Id.ToString(), Is.EqualTo(OfferId));
            Assert.That(offer.InternalId, Is.EqualTo(7));
            Assert.That(offer.Root.ToString(), Is.EqualTo(RootId));
            Assert.That(offer.ItemsCost, Is.EqualTo(12.5));
            Assert.That(offer.RequirementsCost, Is.EqualTo(1500.0));
            Assert.That(offer.SummaryCost, Is.EqualTo(1500.0));
            Assert.That(offer.StartTime, Is.EqualTo(1_700_000_000L));
            Assert.That(offer.EndTime, Is.EqualTo(1_700_003_600L));
            Assert.That(offer.LoyaltyLevel, Is.EqualTo(3));
            Assert.That(offer.SellInOnePiece, Is.True);
            Assert.That(offer.Locked, Is.False);
            Assert.That(offer.Quantity, Is.EqualTo(9));
            Assert.That(offer.CreatedBy, Is.EqualTo(OfferCreator.FakePlayer));

            Assert.That(offer.User.Id.ToString(), Is.EqualTo(UserId));
            Assert.That(offer.User.Nickname, Is.EqualTo("Nikita"));
            Assert.That(offer.User.Rating, Is.EqualTo(0.5));
            Assert.That(offer.User.MemberType, Is.EqualTo(MemberCategory.Sherpa));
            Assert.That(offer.User.Avatar, Is.EqualTo("/files/trader/avatar/x.jpg"));
            Assert.That(offer.User.IsRatingGrowing, Is.True);
            Assert.That(offer.User.Aid, Is.EqualTo(1234));

            Assert.That(offer.Items, Has.Count.EqualTo(1));
            Assert.That(offer.Items![0].Id.ToString(), Is.EqualTo(ItemId));
            Assert.That(offer.Items[0].Template.ToString(), Is.EqualTo(TemplateId));
            Assert.That(offer.Items[0].ParentId, Is.Null);

            var requirement = offer.Requirements!.Single();
            Assert.That(requirement.TemplateId.ToString(), Is.EqualTo(TemplateId));
            Assert.That(requirement.Count, Is.EqualTo(1500.0));
            Assert.That(requirement.OnlyFunctional, Is.True);
            Assert.That(requirement.Level, Is.EqualTo(2));
            Assert.That(requirement.Side, Is.EqualTo(DogtagExchangeSide.Usec));
        });
    }

    [Test]
    public void AnItemUpdRoundTripsThroughTheTranscoder()
    {
        var payload = BuildOfferWithOneItem(
            (ref MessagePackWriter writer) =>
            {
                writer.WriteMapHeader(3);
                writer.Write("_id");
                writer.Write(ItemId);
                writer.Write("_tpl");
                writer.Write(TemplateId);
                writer.Write("upd");
                writer.WriteMapHeader(2);
                writer.Write("StackObjectsCount");
                writer.Write(3);
                writer.Write("sptPresetId");
                writer.Write(PresetId);
            }
        );

        var offer = MsgpackOfferReader.ReadOffer(payload);

        Assert.Multiple(() =>
        {
            Assert.That(offer.Items![0].Upd!.StackObjectsCount, Is.EqualTo(3));
            Assert.That(offer.Items[0].Upd!.SptPresetId!.Value.ToString(), Is.EqualTo(PresetId));
        });
    }

    /// <summary>
    /// The production <c>upd</c> is not flat - <c>randomise_item_condition</c> fills
    /// <c>Repairable</c> and <c>Buff</c> - so the transcoder's map-in-map recursion carries real
    /// traffic.
    /// </summary>
    [Test]
    public void ANestedUpdMapRecursesThroughTheTranscoder()
    {
        var payload = BuildOfferWithOneItem(
            (ref MessagePackWriter writer) =>
            {
                writer.WriteMapHeader(3);
                writer.Write("_id");
                writer.Write(ItemId);
                writer.Write("_tpl");
                writer.Write(TemplateId);
                writer.Write("upd");
                writer.WriteMapHeader(2);
                writer.Write("Repairable");
                writer.WriteMapHeader(2);
                writer.Write("Durability");
                writer.Write(79.5);
                writer.Write("MaxDurability");
                writer.Write(100);
                writer.Write("SpawnedInSession");
                writer.Write(true);
            }
        );

        var offer = MsgpackOfferReader.ReadOffer(payload);

        Assert.Multiple(() =>
        {
            Assert.That(offer.Items![0].Upd!.Repairable!.Durability, Is.EqualTo(79.5));
            Assert.That(offer.Items[0].Upd!.Repairable!.MaxDurability, Is.EqualTo(100));
            Assert.That(offer.Items[0].Upd!.SpawnedInSession, Is.True);
        });
    }

    /// <summary>
    /// Every remaining transcoder arm in one value: an array, a float, a boolean, a nested map and
    /// a nil both as a member and inside a container.
    /// </summary>
    [Test]
    public void AStructuredLocationTranscodesEveryScalarArm()
    {
        var payload = BuildOfferWithOneItem(
            (ref MessagePackWriter writer) =>
            {
                writer.WriteMapHeader(3);
                writer.Write("_id");
                writer.Write(ItemId);
                writer.Write("_tpl");
                writer.Write(TemplateId);
                writer.Write("location");
                writer.WriteMapHeader(4);
                writer.Write("rotation");
                writer.Write(true);
                writer.Write("offset");
                writer.Write(2.5);
                writer.Write("isSearched");
                writer.WriteNil();
                writer.Write("cells");
                writer.WriteArrayHeader(4);
                writer.Write(-1);
                writer.Write(0.25);
                writer.Write(false);
                writer.WriteNil();
            }
        );

        var offer = MsgpackOfferReader.ReadOffer(payload);

        var location = (JsonElement)offer.Items![0].Location!;
        var cells = location.GetProperty("cells");
        Assert.Multiple(() =>
        {
            Assert.That(location.GetProperty("rotation").GetBoolean(), Is.True);
            Assert.That(location.GetProperty("offset").GetDouble(), Is.EqualTo(2.5));
            Assert.That(location.GetProperty("isSearched").ValueKind, Is.EqualTo(JsonValueKind.Null));
            Assert.That(cells.GetArrayLength(), Is.EqualTo(4));
            Assert.That(cells[0].GetInt32(), Is.EqualTo(-1));
            Assert.That(cells[1].GetDouble(), Is.EqualTo(0.25));
            Assert.That(cells[2].GetBoolean(), Is.False);
            Assert.That(cells[3].ValueKind, Is.EqualTo(JsonValueKind.Null));
        });
    }

    /// <summary>
    /// <c>bin</c> has no JSON spelling and nothing on the Rust side emits it, so it is a wire
    /// contract violation rather than something to transcode.
    /// </summary>
    [Test]
    public void ABinaryValueIsAWireContractViolation()
    {
        var payload = BuildOfferWithOneItem(
            (ref MessagePackWriter writer) =>
            {
                writer.WriteMapHeader(3);
                writer.Write("_id");
                writer.Write(ItemId);
                writer.Write("_tpl");
                writer.Write(TemplateId);
                writer.Write("location");
                writer.Write(new byte[] { 1, 2, 3 });
            }
        );

        var thrown = Assert.Throws<InvalidOperationException>(() => MsgpackOfferReader.ReadOffer(payload));
        Assert.That(thrown!.Message, Does.Contain("Binary"));
    }

    [Test]
    public void AnIntegerLocationBecomesAJsonElementNumber()
    {
        var payload = BuildOfferWithOneItem(
            (ref MessagePackWriter writer) =>
            {
                writer.WriteMapHeader(3);
                writer.Write("_id");
                writer.Write(ItemId);
                writer.Write("_tpl");
                writer.Write(TemplateId);
                writer.Write("location");
                writer.Write(5);
            }
        );

        var offer = MsgpackOfferReader.ReadOffer(payload);

        Assert.That(((JsonElement)offer.Items![0].Location!).GetInt32(), Is.EqualTo(5));
    }

    /// <summary>
    /// The transcoder's buffer and the frame scratch are reused per thread, so a value materialized
    /// from an earlier frame must not be reading bytes a later one overwrote.
    /// </summary>
    [Test]
    public void AMaterializedValueSurvivesTheNextFrameReusingTheBuffers()
    {
        var first = MsgpackOfferReader.ReadOffer(
            BuildOfferWithOneItem(
                (ref MessagePackWriter writer) =>
                {
                    writer.WriteMapHeader(3);
                    writer.Write("_id");
                    writer.Write(ItemId);
                    writer.Write("_tpl");
                    writer.Write(TemplateId);
                    writer.Write("location");
                    writer.WriteMapHeader(1);
                    writer.Write("x");
                    writer.Write("first");
                }
            )
        );
        var location = (JsonElement)first.Items![0].Location!;

        // Longer in every dimension, so a stale view would read this frame's bytes, not its own
        var second = MsgpackOfferReader.ReadOffer(
            BuildOfferWithOneItem(
                (ref MessagePackWriter writer) =>
                {
                    writer.WriteMapHeader(3);
                    writer.Write("_id");
                    writer.Write(ItemId);
                    writer.Write("_tpl");
                    writer.Write(TemplateId);
                    writer.Write("location");
                    writer.WriteMapHeader(2);
                    writer.Write("x");
                    writer.Write(new string('z', 512));
                    writer.Write("y");
                    writer.Write(new string('q', 512));
                }
            )
        );

        Assert.Multiple(() =>
        {
            Assert.That(location.GetProperty("x").GetString(), Is.EqualTo("first"));
            Assert.That(((JsonElement)second.Items![0].Location!).GetProperty("x").GetString(), Is.EqualTo(new string('z', 512)));
        });
    }

    [Test]
    public void AModAddedItemFieldLandsInExtensionData()
    {
        var extensionData = typeof(Item).GetProperty("ExtensionData");
        if (extensionData is null)
        {
            Assert.Ignore("extension data is Ceciler-injected in Release builds only");
        }

        var payload = BuildOfferWithOneItem(
            (ref MessagePackWriter writer) =>
            {
                writer.WriteMapHeader(3);
                writer.Write("_id");
                writer.Write(ItemId);
                writer.Write("_tpl");
                writer.Write(TemplateId);
                writer.Write("modField");
                writer.Write("kept");
            }
        );

        var offer = MsgpackOfferReader.ReadOffer(payload);

        var kept = (Dictionary<string, object>?)extensionData!.GetValue(offer.Items![0]);
        Assert.That(kept, Is.Not.Null);
        Assert.That(((JsonElement)kept!["modField"]).GetString(), Is.EqualTo("kept"));
    }

    [Test]
    public void AHeaderPayloadParses()
    {
        var payload = Build(
            (ref MessagePackWriter writer) =>
            {
                writer.WriteMapHeader(2);
                writer.Write("rejectedCanSellTemplates");
                writer.WriteArrayHeader(1);
                writer.Write(TemplateId);
                writer.Write("diagnostics");
                writer.WriteArrayHeader(1);
                writer.WriteMapHeader(4);
                writer.Write("level");
                writer.Write("warning");
                writer.Write("localeKey");
                writer.Write("ragfair-unable_to_find_item");
                writer.Write("args");
                writer.WriteMapHeader(1);
                writer.Write("tpl");
                writer.Write(TemplateId);
                writer.Write("message");
                writer.WriteNil();
            }
        );

        var header = MsgpackOfferReader.ReadHeader(payload);

        Assert.Multiple(() =>
        {
            Assert.That(header.RejectedCanSellTemplates.Single().ToString(), Is.EqualTo(TemplateId));
            var diagnostic = header.Diagnostics.Single();
            Assert.That(diagnostic.Level, Is.EqualTo("warning"));
            Assert.That(diagnostic.LocaleKey, Is.EqualTo("ragfair-unable_to_find_item"));
            Assert.That(diagnostic.Message, Is.Null);
            Assert.That(diagnostic.Args!.Value.GetProperty("tpl").GetString(), Is.EqualTo(TemplateId));
        });
    }

    private delegate void WriteAction(ref MessagePackWriter writer);

    private static byte[] Build(WriteAction write)
    {
        var buffer = new ArrayBufferWriter<byte>();
        var writer = new MessagePackWriter(buffer);
        write(ref writer);
        writer.Flush();
        return buffer.WrittenSpan.ToArray();
    }

    /// <summary>
    /// The smallest offer the reader accepts - <c>user</c> plus one caller-shaped item.
    /// </summary>
    private static byte[] BuildOfferWithOneItem(WriteAction writeItem)
    {
        return Build(
            (ref MessagePackWriter writer) =>
            {
                writer.WriteMapHeader(3);
                writer.Write("_id");
                writer.Write(OfferId);
                writer.Write("user");
                WriteUser(ref writer);
                writer.Write("items");
                writer.WriteArrayHeader(1);
                writeItem(ref writer);
            }
        );
    }

    private static void WriteUser(ref MessagePackWriter writer)
    {
        writer.WriteMapHeader(7);
        writer.Write("id");
        writer.Write(UserId);
        writer.Write("nickname");
        writer.Write("Nikita");
        writer.Write("rating");
        writer.Write(0.5);
        writer.Write("memberType");
        writer.Write((int)MemberCategory.Sherpa);
        writer.Write("avatar");
        writer.Write("/files/trader/avatar/x.jpg");
        writer.Write("isRatingGrowing");
        writer.Write(true);
        writer.Write("aid");
        writer.Write(1234);
    }
}
