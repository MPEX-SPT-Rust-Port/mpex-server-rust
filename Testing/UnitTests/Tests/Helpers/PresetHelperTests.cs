using NUnit.Framework;
using SPTarkov.Server.Core.Helpers.Items;

namespace UnitTests.Tests.Helpers;

[TestFixture]
public class PresetHelperTests
{
    private PresetHelper _presetHelper = default!;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        _presetHelper = DI.GetInstance().GetService<PresetHelper>();
    }

    /// <summary>
    /// The bulk lookup is only a valid substitute for the per-tpl one if it resolves every tpl the
    /// same way, fallback included
    /// </summary>
    [Test]
    public void GetDefaultPresetByTplAgreesWithGetDefaultPresetForEveryTpl()
    {
        var byTpl = _presetHelper.GetDefaultPresetByTpl();

        Assert.That(byTpl, Is.Not.Empty);
        Assert.Multiple(() =>
        {
            foreach (var (templateId, preset) in byTpl)
            {
                Assert.That(preset.Id, Is.EqualTo(_presetHelper.GetDefaultPreset(templateId)?.Id), $"tpl {templateId}");
            }
        });
    }
}
