using SPTarkov.DI.Annotations;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Config;

namespace SPTarkov.Server.Core.Helpers.Commerce;

[Injectable(InjectionType.Singleton)]
public class PaymentHelper(InventoryConfig inventoryConfig)
{
    protected bool AddedCustomMoney;
    protected readonly HashSet<MongoId> MoneyTpls = [Money.DOLLARS, Money.EUROS, Money.ROUBLES, Money.GP];

    /// <summary>
    ///     Exposed so the native ragfair projection can ship the same <c>customMoneyTpls</c> set
    ///     <see cref="IsMoneyTpl"/> unions in, without a constructor change on its caller.
    /// </summary>
    internal InventoryConfig InventoryConfig
    {
        get { return inventoryConfig; }
    }

    /// <summary>
    ///     Is the passed in tpl money (also checks custom currencies in inventoryConfig.customMoneyTpls)
    /// </summary>
    /// <param name="tpl">Item Tpl to check</param>
    /// <returns></returns>
    public bool IsMoneyTpl(MongoId tpl)
    {
        // Add custom currency first time this method is accessed
        if (!AddedCustomMoney)
        {
            foreach (var customMoney in inventoryConfig.CustomMoneyTpls)
            {
                MoneyTpls.Add(customMoney);
            }

            AddedCustomMoney = true;
        }

        return MoneyTpls.Contains(tpl);
    }
}
