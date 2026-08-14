//! `Generators/Ragfair/RagfairOfferGenerator.cs` — the batch pass over a set of assorts
//! ([`generate_dynamic_offers`]), the barter schemes each offer is priced with, the offer object
//! itself, and the condition randomisation and armor-plate removal its items go through.
//!
//! **The offer-object draws**, in the order [`create_offer`] spends them:
//!
//! | step | draws |
//! |---|---|
//! | the requirement mapping and the ammo-box hydration | **0** |
//! | [`create_user_data_for_flea_offer`], trader | **0** |
//! | [`create_user_data_for_flea_offer`], fake player | **5** — faction, nickname, rating, rating growth, account id |
//! | [`get_offer_end_time`], fake player | **1** |
//!
//! The id is a [`mongo_id`], which is drawn outside the seeded stream in both languages.
//!
//! **The barter-scheme draws.** [`create_currency_barter_scheme`] costs one weighted currency draw
//! plus one biased price draw. [`create_barter_barter_scheme`] costs a price draw of its own, and
//! then either an item-count draw plus one index draw, or a **whole second currency scheme** —
//! both of its fall-throughs re-price the offer rather than reusing the price they already have.
//!
//! **The condition draw table.** One `randomise_offer_item_upd_properties` call per item class, in
//! call order. `add_missing_conditions` runs first and always, and never draws.
//!
//! | item class | draws |
//! |---|---|
//! | any, `offerCreator != FakePlayer` | **0** |
//! | any, no `dynamic.condition` base-class match | **0** |
//! | any matched tpl | **1** — [`get_chance_100`] on `conditionChance * 100`; a failed roll stops here |
//! | …then armor (`ArmorItemCanHoldMods` / plate / armored equipment) | **2** + `4 × (children with armorClass > 1)`, plus **1** if a visor child is found and **2** if its 25% roll then passes |
//! | …then weapon | **2 + 4** |
//! | …then medkit / key / food-drink / repair kit | **2** |
//! | …then fuel | **2 + 1** |
//! | …then nothing matched | **2** |
//!
//! The leading **2** is `RandomiseItemCondition`'s pair of multiplier draws at `:699-700`, and the
//! first of them is `GetDouble(Max.Min, Max.Min)` — the `Max.Min` is read *twice*. That degenerate
//! range always returns `Max.Min`, but `RandomUtil.GetDouble` (`:76-80`) still calls the generator,
//! so the draw is spent. Transcribed as-is; it is not a typo to fix here.
//!
//! [`remove_armor_plates`] costs **1** draw unconditionally — the `GetChance100` at `:512` is
//! evaluated before the `ArmorItemHasRemovablePlateSlots` gate, so an armor with no removable plate
//! slots still spends it. [`remove_banned_plates_from_preset`] draws **0**.
//!
//! Where the C# would throw, and what happens here instead:
//! - `AddMissingConditions` (`:837`) dereferences `GetItem(...).Value.Properties` unguarded — a tpl
//!   the items view does not know becomes a [`LootError`];
//! - `Condition[id]` (`:661`, `:698`) is a `Dictionary` indexer — a missing key becomes a
//!   [`LootError`] carrying the `KeyNotFoundException` text. The one reachable caller takes the id
//!   straight off that dictionary's keys, so it cannot miss;
//! - the remaining unguarded dereferences on this path (`Upd`/`Upd.Repairable` being null when
//!   written at `:794`/`:846`, and the `(double)` casts of a null `MaxDurability`, `MaxResource`,
//!   `MaxRepairResource` or `MedKit.HpResource`) have no error channel in the C# signatures the
//!   port mirrors: a missing `Upd` is materialised the way `AddUpd` would, and a missing numeric
//!   property reads as `0`. Every one of them needs a template that omits the property its own
//!   branch selected on, which real data never does.

use std::collections::HashSet;
use std::time::Instant;

use indexmap::IndexSet;
use serde_json::json;

use super::{RagfairContext, plain};
use crate::loot::item_helper::{
    AMMO_BOX, ARMOR_PLATE, ARMORED_EQUIPMENT, FUEL, LootError, WEAPON, add_cartridges_to_ammo_box,
    armor_item_can_hold_mods, armor_item_has_removable_plate_slots, get_item,
    get_removable_plate_slot_ids, is_of_baseclass, is_of_baseclasses, reparent_item_and_children,
};
use crate::loot::models::{
    Item, UpdFoodDrink, UpdKey, UpdMedKit, UpdRepairKit, UpdRepairable, UpdResource,
};
use crate::loot::mongo_id;
use crate::loot::random_util::{
    TestSeedGuard, generate_account_id, get_array_value, get_bool, get_chance_100, get_double,
    get_int, round_half_even, round_to_digits,
};
use crate::ragfair::assort_generator::generate_ragfair_assort_items;
use crate::ragfair::models::{
    ArmorPlateBlacklistSettingsWire, BarterDetailsWire, DynamicOffersResult,
    GenerateDynamicOffersRequest, OfferRequirementWire, RagfairOfferUserWire, RagfairOfferWire,
};
use crate::ragfair::price_service::{
    DOLLARS, EUROS, GP, ROUBLES, get_dynamic_offer_price_for_offer, get_flea_price_for_item,
    get_static_price_for_item, spt_preset_id,
};
use crate::ragfair::server_helper::{
    calculate_dynamic_stack_count, get_dynamic_offer_currency, get_offer_count_by_base_type,
    is_item_valid_ragfair_item,
};

/// `Models/Enums/OfferCreator.cs` — the wire never carries it; it is a call-site constant on the
/// C# side too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfferCreator {
    Player,
    Trader,
    FakePlayer,
}

/// `Models/Spt/Ragfair/CreateFleaOfferDetails.cs`, whole.
pub struct CreateFleaOfferDetails {
    pub user_id: String,
    pub time: i64,
    pub items: Vec<Item>,
    pub barter_scheme: Vec<BarterScheme>,
    pub loyal_level: i32,
    pub quantity: i32,
    pub creator: OfferCreator,
    pub sell_in_one_piece: bool,
}

/// `Models/Eft/Common/Tables/Trader.cs:254-276` `BarterScheme`, minus `sptQuestLocked`, which
/// nothing on this path reads or writes.
#[derive(Debug, Default)]
pub struct BarterScheme {
    pub count: f64,
    pub template: String,
    pub only_functional: Option<bool>,
    pub level: Option<i32>,
    pub side: Option<i32>,
}

/// `Models/Spt/Ragfair/TplWithFleaPrice.cs`. The tpl is borrowed straight out of the flea price
/// map, which outlives every offer built from it.
pub struct TplWithFleaPrice<'a> {
    pub tpl: &'a str,
    pub price: f64,
}

/// `PaymentHelper.IsMoneyTpl` (`PaymentHelper.cs:19-32`) over the four `Money` constants.
///
/// `inventoryConfig.CustomMoneyTpls` is not on the wire, so a mod-added currency prices through
/// the item arm of [`convert_offer_requirements_into_roubles`] here instead of the money arm. No
/// stock configuration carries one.
fn is_money_tpl(tpl: &str) -> bool {
    [ROUBLES, EUROS, DOLLARS, GP].contains(&tpl)
}

/// `RagfairOfferGenerator.CreateOffer` (`:85-143`). `CreateAndAddFleaOffer` (`:66-77`) is not
/// ported: its two extra statements set `CreatedBy` (which the wire type does not carry — the C#
/// caller stamps it) and push the offer into `RagfairOfferService`.
///
/// The C# hydrates `details.Items` in place, so its caller's list grows too; here the items are
/// cloned into the offer and only the copy is hydrated. Nothing reads the list after the call
/// (`:487-499`), so the two are equivalent.
///
/// `CreatePlayerUserDataForFleaOffer` (`:177-191`) is not ported — it reads a profile that does
/// not cross the FFI boundary — so a `Player` offer is an error rather than a wrong user block.
/// `GenerateDynamicOffers` only ever asks for `FakePlayer` offers.
///
/// # Errors
///
/// An empty item list (`rootItem.Id` is an unguarded deref in the C#), a `Player` creator, and
/// whatever [`create_user_data_for_flea_offer`] and [`get_offer_end_time`] propagate.
pub fn create_offer(
    ctx: &mut RagfairContext,
    details: &CreateFleaOfferDetails,
    offer_counter: &mut i32,
) -> Result<RagfairOfferWire, LootError> {
    let offer_requirements: Vec<OfferRequirementWire> = details
        .barter_scheme
        .iter()
        .map(|barter| {
            let mut offer_requirement = OfferRequirementWire {
                template_id: barter.template.clone(),
                count: round_to_digits(barter.count, 2),
                only_functional: barter.only_functional.unwrap_or(false),
                level: None,
                side: None,
            };

            // Dogtags define level and side
            if barter.level.is_some() {
                offer_requirement.level = barter.level;
                offer_requirement.side = barter.side;
            }

            offer_requirement
        })
        .collect();

    let mut items = details.items.clone();

    // Hydrate ammo boxes with cartridges + ensure only 1 item is present (ammo box)
    // On offer refresh don't re-add cartridges to ammo box that already has cartridges
    if items.len() == 1 && is_of_baseclass(ctx.items, &items[0].template, AMMO_BOX) {
        let ammo_box_tpl = items[0].template.clone();
        if let Err(diagnostic) = add_cartridges_to_ammo_box(ctx.items, &mut items, &ammo_box_tpl) {
            ctx.diagnostics.push(diagnostic);
        }
    }

    let rouble_listing_price = round_half_even(convert_offer_requirements_into_roubles(
        ctx,
        &offer_requirements,
    ));
    let single_item_listing_price = if details.sell_in_one_piece {
        rouble_listing_price / f64::from(details.quantity)
    } else {
        rouble_listing_price
    };

    let user = match details.creator {
        OfferCreator::Player => {
            return Err(LootError::new(
                "A player offer needs the seller's profile, which is not on the wire",
            ));
        }
        creator => {
            create_user_data_for_flea_offer(ctx, &details.user_id, creator == OfferCreator::Trader)?
        }
    };
    let root_item = items
        .first()
        .ok_or_else(|| LootError::new("Object reference not set to an instance of an object."))?;

    let offer = RagfairOfferWire {
        id: mongo_id::generate(),
        internal_id: *offer_counter,
        user,
        root: root_item.id.clone(),
        // Handbook price
        items_cost: round_half_even(get_static_price_for_item(ctx, &root_item.template)),
        requirements: offer_requirements,
        requirements_cost: round_half_even(single_item_listing_price),
        summary_cost: rouble_listing_price,
        start_time: details.time,
        end_time: get_offer_end_time(ctx, details.creator, &details.user_id, details.time)?,
        loyalty_level: details.loyal_level,
        sell_in_one_piece: details.sell_in_one_piece,
        locked: false,
        quantity: details.quantity,
        items,
    };

    *offer_counter += 1;

    Ok(offer)
}

/// `RagfairOfferGenerator.CreateUserDataForFleaOffer` (`:151-170`), with
/// `BotHelper.GetPmcNicknameOfMaxLength` (`BotHelper.cs:136-142`) folded in: the name pools reach
/// this port already gathered and length-filtered, so only its faction draw is left.
///
/// Five draws for a fake player, in source order: faction, nickname, rating, rating growth,
/// account id. A trader draws nothing.
///
/// The trader arm leaves `rating`, `isRatingGrowing` and `aid` at `0`/`false` where the C# record
/// leaves them null. Unreachable from `GenerateDynamicOffers`, which only builds fake-player
/// offers.
///
/// # Errors
///
/// Where `RandomUtil.GetRandomElement` throws: the drawn faction's name pool is empty.
pub fn create_user_data_for_flea_offer(
    ctx: &RagfairContext,
    user_id: &str,
    is_trader: bool,
) -> Result<RagfairOfferUserWire, LootError> {
    // Trader offer
    if is_trader {
        return Ok(RagfairOfferUserWire {
            id: user_id.to_owned(),
            nickname: None,
            rating: 0.0,
            // MemberCategory.Trader
            member_type: 4,
            avatar: None,
            is_rating_growing: false,
            aid: 0,
        });
    }

    // 'Fake' pmc offer
    let names = if get_int(0, 1) == 0 {
        ctx.pmc_names_usec
    } else {
        ctx.pmc_names_bear
    };
    if names.is_empty() {
        return Err(LootError::new(
            "The collection is empty, unable to get a random element",
        ));
    }

    Ok(RagfairOfferUserWire {
        id: user_id.to_owned(),
        nickname: Some(get_array_value(names).clone()),
        rating: get_double(ctx.dynamic.rating.min, ctx.dynamic.rating.max),
        // MemberCategory.Default
        member_type: 0,
        is_rating_growing: get_bool(),
        avatar: None,
        aid: generate_account_id(),
    })
}

/// `RagfairOfferGenerator.ConvertOfferRequirementsIntoRoubles` (`:198-205`). The money arm rounds
/// and the item arm does not — that asymmetry is the C#'s.
pub fn convert_offer_requirements_into_roubles(
    ctx: &RagfairContext,
    offer_requirements: &[OfferRequirementWire],
) -> f64 {
    offer_requirements
        .iter()
        .map(|requirement| {
            if is_money_tpl(&requirement.template_id) {
                round_half_even(calculate_rouble_price(
                    ctx,
                    requirement.count,
                    &requirement.template_id,
                ))
            } else {
                get_flea_price_for_item(ctx, &requirement.template_id) * requirement.count
            }
        })
        .sum()
}

/// `RagfairOfferGenerator.CalculateRoublePrice` (`:229-237`), i.e. `HandbookHelper.InRoubles`
/// (`HandbookHelper.cs:202-207`).
pub fn calculate_rouble_price(
    ctx: &RagfairContext,
    currency_count: f64,
    currency_type: &str,
) -> f64 {
    if currency_type == ROUBLES {
        return currency_count;
    }

    round_half_even(currency_count * get_static_price_for_item(ctx, currency_type))
}

/// `RagfairOfferGenerator.GetOfferEndTime` (`:269-287`). Only the fake-player arm is reachable
/// from `GenerateDynamicOffers`; the other two are ported for shape and error out on the inputs
/// that never cross the FFI boundary — the globals' `offerDurationTimeInHour` for a player offer,
/// the trader table's `nextResupply` for a trader one.
///
/// # Errors
///
/// A `Player` or `Trader` creator, per above.
pub fn get_offer_end_time(
    ctx: &RagfairContext,
    creator_type: OfferCreator,
    _user_id: &str,
    time: i64,
) -> Result<i64, LootError> {
    match creator_type {
        OfferCreator::Player => Err(LootError::new(
            "A player offer's end time needs globals' ragFair.offerDurationTimeInHour, which is not on the wire",
        )),
        OfferCreator::Trader => Err(LootError::new(
            "A trader offer's end time needs the trader's nextResupply, which is not on the wire",
        )),
        OfferCreator::FakePlayer => {
            let random_spread = get_double(
                f64::from(ctx.dynamic.end_time_seconds.min),
                f64::from(ctx.dynamic.end_time_seconds.max),
            );

            // Fake-player offer
            Ok(round_half_even(time as f64 + random_spread) as i64)
        }
    }
}

/// `RagfairOfferGenerator.GenerateDynamicOffers` (`:293-324`) — one whole dynamic pass: the assort
/// walk (or the expired offers handed in), then an offer batch per assort entry.
///
/// **The `Task.Factory.StartNew` fan-out (`:309-317`) is deliberately not reproduced.** The walk
/// here is sequential. Production draws come from a crypto-random generator and the legacy
/// interleaving is nondeterministic anyway, so the parallel version has no output the sequential one
/// lacks — and one thread is what makes a seeded run reproducible for the parity fixtures.
///
/// Natively this only *builds* offers: the C# `CreateAndAddFleaOffer` also stamps `CreatedBy` and
/// inserts into `RagfairOfferService`, both of which stay in the caller's loop.
///
/// **The draw sequence for one plain item** — not a preset, not a pack, not a barter — which is
/// what the parity fixtures pin. A "price draw" below is one biased-random draw
/// ([`get_biased_random_number`](crate::loot::random_util::get_biased_random_number)), which is
/// itself `2 * attempts` raw `next_double48` draws unless the range is degenerate, per
/// [`price_service`](super::price_service)'s own checklist.
///
/// | step | draws |
/// |---|---|
/// | [`get_offer_count_by_base_type`] | **1** [`get_int`] (none for a degenerate range) |
/// | …then, per offer: [`calculate_dynamic_stack_count`] | **1** |
/// | [`remove_armor_plates`] — armor only, and never for an expired offer | **1** |
/// | the barter chance roll | **1** |
/// | the pack chance roll, **skipped when the barter roll won** | **1** |
/// | [`randomise_offer_item_upd_properties`] | the condition table above |
/// | [`create_currency_barter_scheme`] | **1** currency + **1** price per priced item (inserts skipped, weapon-preset break) |
/// | [`create_offer`] | **5** user-block + **1** end-time |
///
/// The two `Stopwatch` debug lines become diagnostics carrying the same text, timed with an
/// [`Instant`]. The first keeps its `> 0` guard; the `IsLogEnabled` gates are the C# caller's, which
/// decides whether to replay a diagnostic at all.
///
/// # Errors
///
/// Whatever the assort walk and the per-offer path propagate.
pub fn generate_dynamic_offers(
    request: GenerateDynamicOffersRequest,
) -> Result<DynamicOffersResult, LootError> {
    let _seed_guard = request.test_seed.map(TestSeedGuard::install);

    let GenerateDynamicOffersRequest {
        timestamp,
        offer_counter_start,
        expired_offers,
        dynamic,
        item_presets,
        default_presets,
        default_presets_by_tpl,
        presets_by_tpl,
        flea_prices,
        handbook_prices,
        highest_trader_prices,
        config_blacklist,
        seasonal_event_active,
        seasonal_item_tpl_blacklist,
        pmc_names_usec,
        pmc_names_bear,
        items,
        ..
    } = request;
    let config_blacklist: HashSet<String> = config_blacklist.into_iter().collect();
    let seasonal_item_tpl_blacklist: HashSet<String> =
        seasonal_item_tpl_blacklist.into_iter().collect();

    let mut ctx = RagfairContext {
        items: &items,
        dynamic: &dynamic,
        item_presets: &item_presets,
        default_presets: &default_presets,
        default_presets_by_tpl: &default_presets_by_tpl,
        presets_by_tpl: &presets_by_tpl,
        flea_prices: &flea_prices,
        handbook_prices: &handbook_prices,
        highest_trader_prices: &highest_trader_prices,
        config_blacklist: &config_blacklist,
        seasonal_item_tpl_blacklist: &seasonal_item_tpl_blacklist,
        pmc_names_usec: &pmc_names_usec,
        pmc_names_bear: &pmc_names_bear,
        timestamp,
        seasonal_event_active,
        diagnostics: Vec::new(),
    };

    let replacing_expired_offers = expired_offers
        .as_ref()
        .is_some_and(|offers| !offers.is_empty());

    let stopwatch = Instant::now();
    // get assort items from param if they exist, otherwise grab freshly generated assorts
    let mut assort_items_to_process = if replacing_expired_offers {
        expired_offers.unwrap_or_default()
    } else {
        generate_ragfair_assort_items(&ctx)?
    };
    let elapsed = stopwatch.elapsed().as_millis();
    if elapsed > 0 {
        ctx.diagnostics.push(plain(
            "debug",
            format!(
                "Took {elapsed}ms to GetRagfairAssorts - {} items",
                assort_items_to_process.len()
            ),
        ));
    }

    let stopwatch = Instant::now();
    let mut offers = Vec::new();
    let mut rejected = IndexSet::new();
    let mut offer_counter = offer_counter_start;
    for assort_item_with_children in &mut assort_items_to_process {
        create_offers_from_assort(
            &mut ctx,
            assort_item_with_children,
            replacing_expired_offers,
            &mut offers,
            &mut rejected,
            &mut offer_counter,
        )?;
    }
    ctx.diagnostics.push(plain(
        "debug",
        format!(
            "Took {}ms to CreateOffersFromAssort",
            stopwatch.elapsed().as_millis()
        ),
    ));

    Ok(DynamicOffersResult {
        offers,
        rejected_can_sell_templates: rejected.into_iter().collect(),
        diagnostics: ctx.diagnostics,
    })
}

/// `RagfairOfferGenerator.CreateOffersFromAssort` (`:332-373`). The C# `config` parameter is not
/// ported: the body never reads it, going to `ragfairConfig.Dynamic` directly instead.
///
/// The list is taken by `&mut` because `RemoveBannedPlatesFromPreset` mutates the *shared* assort
/// entry, before the offer loop clones it — so every offer of one preset loses the same plates.
///
/// # Errors
///
/// An empty list (`rootItem.Template` is an unguarded deref in the C#), a root template the items
/// view does not know, and whatever the per-offer path propagates.
fn create_offers_from_assort(
    ctx: &mut RagfairContext,
    assort_item_with_children: &mut Vec<Item>,
    is_expired_offer: bool,
    offers: &mut Vec<RagfairOfferWire>,
    rejected: &mut IndexSet<String>,
    offer_counter: &mut i32,
) -> Result<(), LootError> {
    let root_tpl = assort_item_with_children
        .first()
        .map(|root_item| root_item.template.clone())
        .ok_or_else(|| LootError::new("Object reference not set to an instance of an object."))?;

    // Only perform checks on newly generated items, skip expired items being refreshed.
    // Short-circuit: an expired offer never runs the validity check, so it never contributes to
    // `rejected` either.
    if !(is_expired_offer || is_item_valid_ragfair_item(ctx, &root_tpl, rejected)) {
        return Ok(());
    }

    // Armor presets can hold plates above the allowed flea level, remove if necessary
    let is_preset = spt_preset_id(&assort_item_with_children[0])
        .is_some_and(|preset_id| ctx.item_presets.contains_key(preset_id));
    let dynamic = ctx.dynamic;
    if !is_expired_offer && is_preset && dynamic.blacklist.enable_bsg_list {
        remove_banned_plates_from_preset(
            ctx,
            assort_item_with_children,
            &dynamic.blacklist.armor_plate,
        );
    }

    // Get number of offers to create
    // Limit to 1 offer when processing expired - like-for-like replacement
    let offer_count = if is_expired_offer {
        1
    } else {
        // `:352` dereferences the `GetItem` result unguarded. Unreachable on a miss in practice:
        // the validity check above already rejects a template the view does not know.
        let Some(item_to_sell_details) = ctx.items.get(&root_tpl) else {
            return Err(LootError::new(
                "Object reference not set to an instance of an object.",
            ));
        };
        let parent = item_to_sell_details.parent.clone().unwrap_or_default();

        get_offer_count_by_base_type(ctx, &parent)?
    };

    for _ in 0..offer_count {
        // Clone the item so we don't have shared references and generate new item IDs
        let mut cloned_assort = assort_item_with_children.clone();
        // C# hands `ReparentItemAndChildren` the very item it is about to overwrite slot 0 with;
        // this port needs the root as a separate value, so it snapshots it first. The only
        // difference — the snapshot misses the root's own re-parenting — is erased two lines below.
        let cloned_root = cloned_assort[0].clone();
        let mut cloned_assort = reparent_item_and_children(&cloned_root, &mut cloned_assort);

        // Clear unnecessary properties
        cloned_assort[0].parent_id = None;
        cloned_assort[0].slot_id = None;

        create_single_offer_for_item(
            ctx,
            &mongo_id::generate(),
            cloned_assort,
            is_preset,
            &root_tpl,
            is_expired_offer,
            OfferCreator::FakePlayer,
            offers,
            offer_counter,
        )?;
    }

    Ok(())
}

/// `RagfairOfferGenerator.CreateSingleOfferForItem` (`:427-501`). The C# takes the raw
/// `TemplateItem`; the only thing the body does with it is hand it to
/// [`randomise_offer_item_upd_properties`], which takes the tpl here, so the tpl is the parameter.
///
/// The item list is taken by value: it is a per-offer clone that nothing reads afterwards, so it
/// moves into the offer rather than being cloned a second time.
///
/// **The draw order is the contract** — stack count, then the plate roll, then the barter roll,
/// then the pack roll. The pack roll sits behind `!isBarterOffer` in a short-circuiting `&&` chain,
/// so a barter win skips it entirely and everything after shifts by a draw.
///
/// # Errors
///
/// An empty item list (`rootItem.Template` is an unguarded deref in the C#), plus whatever the
/// stack count, the barter schemes and [`create_offer`] propagate.
#[expect(
    clippy::too_many_arguments,
    reason = "the C# method's six parameters plus the two accumulators the caller's loop owns"
)]
fn create_single_offer_for_item(
    ctx: &mut RagfairContext,
    seller_id: &str,
    mut item_with_children: Vec<Item>,
    is_preset: bool,
    item_to_sell_tpl: &str,
    is_expired_offer: bool,
    offer_creator: OfferCreator,
    offers: &mut Vec<RagfairOfferWire>,
    offer_counter: &mut i32,
) -> Result<(), LootError> {
    let root_tpl = item_with_children
        .first()
        .map(|root_item| root_item.template.clone())
        .ok_or_else(|| LootError::new("Object reference not set to an instance of an object."))?;

    // Get randomised amount to list on flea
    let mut desired_stack_size = calculate_dynamic_stack_count(ctx, &root_tpl, is_preset)?;

    // Reset stack count to 1 from whatever it was prior
    item_with_children[0]
        .upd
        .get_or_insert_default()
        .stack_objects_count = Some(1.0);

    if !is_expired_offer && armor_item_can_hold_mods(ctx.items, &root_tpl) {
        // Run randomised chance to remove removable plates from new offers(not expired)
        remove_armor_plates(ctx, &mut item_with_children);
    }

    let dynamic = ctx.dynamic;
    let is_barter_offer = get_chance_100(dynamic.barter.chance_percent);
    let is_pack_offer = !is_barter_offer
        && get_chance_100(dynamic.pack.chance_percent)
        && item_with_children.len() == 1
        && is_of_baseclasses(
            ctx.items,
            &root_tpl,
            &dynamic
                .pack
                .item_type_whitelist
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        );

    let barter_scheme = if is_pack_offer {
        // Set pack size
        desired_stack_size = get_int(dynamic.pack.item_count_min, dynamic.pack.item_count_max);

        // Don't randomise pack items
        create_currency_barter_scheme(
            ctx,
            &item_with_children,
            is_pack_offer,
            f64::from(desired_stack_size),
        )?
    } else if is_barter_offer {
        // Apply randomised properties
        randomise_offer_item_upd_properties(
            ctx,
            &mut item_with_children,
            item_to_sell_tpl,
            offer_creator,
        )?;
        let barter_scheme = create_barter_barter_scheme(ctx, &item_with_children, &dynamic.barter)?;
        // The C# only resets an `Upd` that is already there; it never materialises one.
        if dynamic.barter.make_single_stack_only
            && let Some(upd) = item_with_children
                .first_mut()
                .and_then(|root_item| root_item.upd.as_mut())
        {
            upd.stack_objects_count = Some(1.0);
        }

        barter_scheme
    } else {
        // Not barter or pack offer
        // Apply randomised properties
        randomise_offer_item_upd_properties(
            ctx,
            &mut item_with_children,
            item_to_sell_tpl,
            offer_creator,
        )?;
        create_currency_barter_scheme(ctx, &item_with_children, false, 1.0)?
    };

    let create_offer_details = CreateFleaOfferDetails {
        user_id: seller_id.to_owned(),
        // `:491` re-reads the clock per offer; one timestamp for the batch is the sanctioned
        // divergence documented on `GenerateDynamicOffersRequest`.
        time: ctx.timestamp,
        items: item_with_children,
        barter_scheme,
        loyal_level: 1,
        quantity: desired_stack_size,
        creator: offer_creator,
        // sellAsOnePiece - pack offer
        sell_in_one_piece: is_pack_offer,
    };

    offers.push(create_offer(ctx, &create_offer_details, offer_counter)?);

    Ok(())
}

/// `RagfairOfferGenerator.RandomiseOfferItemUpdProperties` (`:641-666`). The C# `userId` parameter
/// is not read by the method body, so it is not a parameter here.
///
/// `itemWithMods` is a slice rather than a `Vec`: nothing below this call resizes the list.
///
/// # Errors
///
/// Propagates [`add_missing_conditions`]'s and [`randomise_item_condition`]'s.
pub fn randomise_offer_item_upd_properties(
    ctx: &RagfairContext,
    item_with_mods: &mut [Item],
    item_details_tpl: &str,
    offer_creator: OfferCreator,
) -> Result<(), LootError> {
    // Add any missing properties to first item in array
    add_missing_conditions(ctx, &mut item_with_mods[0])?;

    if offer_creator != OfferCreator::FakePlayer {
        return Ok(());
    }

    // No condition details found, don't proceed with modifying item conditions
    let Some(parent_id) = get_dynamic_condition_id_for_tpl(ctx, item_details_tpl) else {
        return Ok(());
    };

    let condition = condition_settings(ctx, &parent_id)?;

    // Roll random chance to randomise item condition
    if get_chance_100(condition.condition_chance * 100.0) {
        randomise_item_condition(ctx, &parent_id, item_with_mods, item_details_tpl)?;
    }

    Ok(())
}

/// `RagfairOfferGenerator.GetDynamicConditionIdForTpl` (`:673-686`) — the first base class in
/// `dynamic.condition`'s **insertion order** the tpl derives from.
fn get_dynamic_condition_id_for_tpl(ctx: &RagfairContext, tpl: &str) -> Option<String> {
    // Get keys from condition config dictionary
    for base_class in ctx.dynamic.condition.keys() {
        if is_of_baseclass(ctx.items, tpl, base_class) {
            return Some(base_class.clone());
        }
    }

    None
}

/// The `Condition[id]` indexer at `:661`/`:698`.
fn condition_settings<'a>(
    ctx: &'a RagfairContext,
    condition_settings_id: &str,
) -> Result<&'a crate::ragfair::models::ConditionWire, LootError> {
    ctx.dynamic
        .condition
        .get(condition_settings_id)
        .ok_or_else(|| {
            LootError::new(format!(
                "The given key '{condition_settings_id}' was not present in the dictionary."
            ))
        })
}

/// `RagfairOfferGenerator.RandomiseItemCondition` (`:694-774`). The branch order is the contract —
/// armor, weapon, medkit, key, food/drink, repair kit, fuel — and every arm but the first returns
/// as soon as it fires.
///
/// # Errors
///
/// A `conditionSettingsId` the config does not carry (`KeyNotFoundException` in C#).
fn randomise_item_condition(
    ctx: &RagfairContext,
    condition_settings_id: &str,
    item_with_mods: &mut [Item],
    item_details_tpl: &str,
) -> Result<(), LootError> {
    let item_condition_values = condition_settings(ctx, condition_settings_id)?;
    // `:699` reads `Max.Min` for both bounds. A degenerate range, and still one draw.
    let max_multiplier = get_double(item_condition_values.max.min, item_condition_values.max.min);
    let current_multiplier = get_double(
        item_condition_values.current.min,
        item_condition_values.current.max,
    );

    let root_tpl = item_with_mods[0].template.clone();

    // Randomise armor + plates + armor related things
    if armor_item_can_hold_mods(ctx.items, &root_tpl)
        || is_of_baseclasses(ctx.items, &root_tpl, &[ARMOR_PLATE, ARMORED_EQUIPMENT])
    {
        randomise_armor_durability_values(ctx, item_with_mods, current_multiplier, max_multiplier);

        // Add hits to visor.
        //
        // Dead branch, ported as written: `:712` compares a child's *parent item id* against the
        // ARMORED_EQUIPMENT *base class tpl*, and a parent id is another item's `_id`, never a
        // base class. No real offer ever satisfies it, so `Upd.FaceShield` is never written here.
        let visor_mod = item_with_mods.iter_mut().find(|item| {
            item.parent_id.as_deref() == Some(ARMORED_EQUIPMENT)
                && item.slot_id.as_deref() == Some("mod_equipment_000")
        });
        if let Some(visor_mod) = visor_mod
            && get_chance_100(25.0)
        {
            let upd = visor_mod.upd.get_or_insert_default();
            // No typed `UpdFaceShield` in this crate; `Upd` serialises its members verbatim.
            upd.extra
                .insert("FaceShield".to_owned(), json!({ "Hits": get_int(1, 3) }));
        }

        return Ok(());
    }

    // Randomise Weapons
    if is_of_baseclass(ctx.items, item_details_tpl, WEAPON) {
        randomise_weapon_durability(
            ctx,
            &mut item_with_mods[0],
            item_details_tpl,
            max_multiplier,
            current_multiplier,
        );

        return Ok(());
    }

    let item_details = get_item(ctx.items, item_details_tpl);
    let root_item = &mut item_with_mods[0];

    if let Some(med_kit) = root_item.upd.as_mut().and_then(|upd| upd.med_kit.as_mut()) {
        // Randomize health
        let hp_resource = round_half_even(med_kit.hp_resource.unwrap_or_default() * max_multiplier);
        med_kit.hp_resource = Some(if hp_resource == 0.0 { 1.0 } else { hp_resource });

        return Ok(());
    }

    let maximum_number_of_usage = item_details.and_then(|details| details.maximum_number_of_usage);
    if let Some(key) = root_item.upd.as_mut().and_then(|upd| upd.key.as_mut())
        && maximum_number_of_usage.is_some_and(|uses| uses > 1)
    {
        // Randomize key uses
        key.number_of_usages = Some(round_half_even(
            f64::from(maximum_number_of_usage.unwrap_or_default()) * (1.0 - max_multiplier),
        ) as i32);

        return Ok(());
    }

    let max_resource = item_details.and_then(|details| details.max_resource);
    if let Some(food_drink) = root_item
        .upd
        .as_mut()
        .and_then(|upd| upd.food_drink.as_mut())
    {
        // randomize food/drink value
        let hp_percent =
            round_half_even(f64::from(max_resource.unwrap_or_default()) * max_multiplier);
        food_drink.hp_percent = Some(if hp_percent == 0.0 { 1.0 } else { hp_percent });

        return Ok(());
    }

    let max_repair_resource = item_details.and_then(|details| details.max_repair_resource);
    if let Some(repair_kit) = root_item
        .upd
        .as_mut()
        .and_then(|upd| upd.repair_kit.as_mut())
    {
        // randomize repair kit (armor/weapon) uses
        let resource = round_half_even(max_repair_resource.unwrap_or_default() * max_multiplier);
        repair_kit.resource = Some(if resource == 0.0 { 1.0 } else { resource });

        return Ok(());
    }

    if is_of_baseclass(ctx.items, item_details_tpl, FUEL) {
        let total_capacity = f64::from(max_resource.unwrap_or_default());

        // Randomise multi between value in config and 1 (100%)
        let randomised_multi = get_double(max_multiplier, 1.0);
        let remaining_fuel = round_half_even(total_capacity * randomised_multi);
        root_item.upd.get_or_insert_default().resource = Some(UpdResource {
            units_consumed: Some(total_capacity - remaining_fuel),
            value: Some(remaining_fuel),
        });
    }

    Ok(())
}

/// `RagfairOfferGenerator.RandomiseWeaponDurability` (`:783-796`) — four draws, and a durability
/// that lands on zero is lifted to one.
fn randomise_weapon_durability(
    ctx: &RagfairContext,
    item: &mut Item,
    item_db_tpl: &str,
    max_multiplier: f64,
    current_multiplier: f64,
) {
    // Max
    let base_max_durability = get_item(ctx.items, item_db_tpl)
        .and_then(|details| details.max_durability)
        .unwrap_or_default();
    let lowest_max_durability = get_double(max_multiplier, 1.0) * base_max_durability;
    let chosen_max_durability =
        round_half_even(get_double(lowest_max_durability, base_max_durability));

    // Current
    let lowest_current_durability = get_double(current_multiplier, 1.0) * chosen_max_durability;
    let chosen_current_durability =
        round_half_even(get_double(lowest_current_durability, chosen_max_durability));

    let repairable = item
        .upd
        .get_or_insert_default()
        .repairable
        .get_or_insert_default();
    // Never var value become 0
    repairable.durability = Some(if chosen_current_durability == 0.0 {
        1.0
    } else {
        chosen_current_durability
    });
    repairable.max_durability = Some(chosen_max_durability);
}

/// `RagfairOfferGenerator.RandomiseArmorDurabilityValues` (`:804-827`) — four draws **per child**
/// whose template has `armorClass > 1`, so the draw count follows the child list. Note the
/// parameter order: `current` first, then `max`, and the first draw uses `max`.
fn randomise_armor_durability_values(
    ctx: &RagfairContext,
    armor_with_mods: &mut [Item],
    current_multiplier: f64,
    max_multiplier: f64,
) {
    for armor_item in armor_with_mods.iter_mut() {
        let item_db_details = get_item(ctx.items, &armor_item.template);
        if item_db_details.is_some_and(|details| details.armor_class.is_some_and(|class| class > 1))
        {
            let upd = armor_item.upd.get_or_insert_default();

            let base_max_durability = item_db_details
                .and_then(|details| details.max_durability)
                .unwrap_or_default();
            let lowest_max_durability = get_double(max_multiplier, 1.0) * base_max_durability;
            let chosen_max_durability =
                round_half_even(get_double(lowest_max_durability, base_max_durability));

            let lowest_current_durability =
                get_double(current_multiplier, 1.0) * chosen_max_durability;
            let chosen_current_durability =
                round_half_even(get_double(lowest_current_durability, chosen_max_durability));

            upd.repairable = Some(UpdRepairable {
                // Never var value become 0
                durability: Some(if chosen_current_durability == 0.0 {
                    1.0
                } else {
                    chosen_current_durability
                }),
                max_durability: Some(chosen_max_durability),
                extra: serde_json::Map::new(),
            });
        }
    }
}

/// `RagfairOfferGenerator.AddMissingConditions` (`:835-877`) — the first matching arm writes and
/// returns. No draws.
///
/// # Errors
///
/// Where `:837` dereferences a `GetItem` miss: a tpl the items view does not know.
fn add_missing_conditions(ctx: &RagfairContext, item: &mut Item) -> Result<(), LootError> {
    let props = get_item(ctx.items, &item.template).ok_or_else(|| {
        LootError::new("Object reference not set to an instance of an object.".to_owned())
    })?;

    let is_repairable = props.durability.is_some();
    let is_medkit = props.max_hp_resource.is_some();
    let is_key = props.maximum_number_of_usage.is_some();
    let is_consumable =
        props.max_resource.is_some_and(|max| max > 1) && props.food_use_time.is_some();
    let is_repair_kit = props.max_repair_resource.is_some();

    if is_repairable && props.durability.is_some_and(|durability| durability > 0.0) {
        item.upd.get_or_insert_default().repairable = Some(UpdRepairable {
            durability: props.durability,
            max_durability: props.durability,
            extra: serde_json::Map::new(),
        });

        return Ok(());
    }

    if is_medkit && props.max_hp_resource.is_some_and(|max| max > 0) {
        item.upd.get_or_insert_default().med_kit = Some(UpdMedKit {
            hp_resource: props.max_hp_resource.map(f64::from),
        });

        return Ok(());
    }

    if is_key {
        item.upd.get_or_insert_default().key = Some(UpdKey {
            number_of_usages: Some(0),
        });

        return Ok(());
    }

    // Food/drink
    if is_consumable {
        item.upd.get_or_insert_default().food_drink = Some(UpdFoodDrink {
            hp_percent: props.max_resource.map(f64::from),
        });

        return Ok(());
    }

    if is_repair_kit {
        item.upd.get_or_insert_default().repair_kit = Some(UpdRepairKit {
            resource: props.max_repair_resource,
        });
    }

    Ok(())
}

/// `RagfairOfferGenerator.RemoveBannedPlatesFromPreset` (`:381-416`). No draws.
///
/// C# iterates a snapshot of the plate slots but removes off the live list by `IndexOf`
/// (`:410`), so each removal shifts the later plates' indexes; collecting the plates by identity
/// first and re-finding each one reproduces that exactly.
pub fn remove_banned_plates_from_preset(
    ctx: &RagfairContext,
    preset_with_children: &mut Vec<Item>,
    plate_settings: &ArmorPlateBlacklistSettingsWire,
) -> bool {
    // Cant hold armor inserts, skip
    if !armor_item_can_hold_mods(ctx.items, &preset_with_children[0].template) {
        return false;
    }

    let plate_slot_ids: Vec<String> = preset_with_children
        .iter()
        .filter(|item| is_plate_slot(item))
        .map(|item| item.id.clone())
        .collect();
    // Has no plate slots e.g. "front_plate", exit
    if plate_slot_ids.is_empty() {
        return false;
    }

    let mut removed_plate = false;
    for plate_id in plate_slot_ids {
        let Some(index) = preset_with_children
            .iter()
            .position(|item| item.id == plate_id)
        else {
            continue;
        };
        let plate_slot = &preset_with_children[index];

        let plate_details = get_item(ctx.items, &plate_slot.template);
        if plate_settings
            .ignore_slots
            .contains(&lowercased_slot_id(plate_slot))
        {
            continue;
        }

        let plate_armor_level = plate_details
            .and_then(|details| details.armor_class)
            .unwrap_or(0);
        if plate_armor_level > plate_settings.max_protection_level {
            preset_with_children.remove(index);
            removed_plate = true;
        }
    }

    removed_plate
}

/// `RagfairOfferGenerator.RemoveArmorPlates` (`:508-528`). The C# takes the root item separately;
/// it is always `itemWithChildren[0]` (`:436`, `:447`).
///
/// The `GetChance100` is drawn **before** the plate-slot gate, so an armor with no removable plate
/// slots still spends it.
pub fn remove_armor_plates(ctx: &RagfairContext, item_with_children: &mut Vec<Item>) {
    let armor_config = &ctx.dynamic.armor;

    let should_remove_plates =
        get_chance_100(f64::from(armor_config.remove_removable_plate_chance));
    if !should_remove_plates
        || !armor_item_has_removable_plate_slots(ctx.items, &item_with_children[0].template)
    {
        return;
    }

    // Latest first, to ensure we don't move later items off by 1 each time we remove an item below
    // it. C# collects the indexes into a `HashSet<int>` and orders it descending, which a
    // descending sort of the (already unique) indexes matches.
    let mut indexes_to_remove: Vec<usize> = item_with_children
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            armor_config
                .plate_slot_id_to_remove_pool
                .contains(&lowercased_slot_id(item))
        })
        .map(|(index, _)| index)
        .collect();
    indexes_to_remove.sort_unstable_by(|left, right| right.cmp(left));

    for index in indexes_to_remove {
        item_with_children.remove(index);
    }
}

/// `RagfairOfferGenerator.CreateBarterBarterScheme` (`:885-927`).
///
/// Both fall-throughs re-enter [`create_currency_barter_scheme`], which draws its own price — so
/// the below-threshold path spends **two** price draws and the no-candidates path **three**. That
/// is legacy behaviour, transcribed rather than fixed.
///
/// # Errors
///
/// An empty item list (`rootOfferItem.Template` is an unguarded deref in the C#), plus whatever
/// the price and currency lookups propagate.
pub fn create_barter_barter_scheme(
    ctx: &mut RagfairContext,
    offer_items: &[Item],
    barter_config: &BarterDetailsWire,
) -> Result<Vec<BarterScheme>, LootError> {
    // Get flea price of item being sold
    let price_of_offer_item = get_dynamic_offer_price_for_offer(ctx, offer_items, ROUBLES, false)?;

    // Don't make items under a designated rouble value into barter offers
    if price_of_offer_item < barter_config.min_rouble_cost_to_become_barter {
        return create_currency_barter_scheme(ctx, offer_items, false, 1.0);
    }

    // Get a randomised number of barter items to list offer for
    let barter_item_count = get_int(barter_config.item_count_min, barter_config.item_count_max);

    // Get desired cost of individual item offer will be listed for e.g. offer = 15k, item count =
    // 3, desired item cost = 5k
    let desired_item_cost_rouble =
        round_half_even(price_of_offer_item / f64::from(barter_item_count));

    // Rouble amount to go above/below when looking for an item (Wiggle cost of item a little)
    let offer_cost_variance_roubles =
        desired_item_cost_rouble * barter_config.price_range_variance_percent / 100.0;

    // List of items and their flea price
    let item_flea_prices = get_flea_prices_as_array(ctx);

    // Filter possible barters to items that match the price range + not itself
    let min = desired_item_cost_rouble - offer_cost_variance_roubles;
    let max = desired_item_cost_rouble + offer_cost_variance_roubles;
    let root_offer_item_tpl = offer_items
        .first()
        .map(|item| item.template.as_str())
        .ok_or_else(|| LootError::new("Object reference not set to an instance of an object."))?;

    let items_inside_price_bounds: Vec<&TplWithFleaPrice> = item_flea_prices
        .iter()
        .filter(|item_and_price| {
            item_and_price.price >= min
                && item_and_price.price <= max
                // Don't allow the item being sold to be chosen
                && item_and_price.tpl != root_offer_item_tpl
        })
        .collect();

    // No items on flea have a matching price, fall back to currency
    if items_inside_price_bounds.is_empty() {
        return create_currency_barter_scheme(ctx, offer_items, false, 1.0);
    }

    // Choose random item from price-filtered flea items
    let random_item = get_array_value(&items_inside_price_bounds);

    Ok(vec![BarterScheme {
        count: f64::from(barter_item_count),
        template: random_item.tpl.to_owned(),
        ..BarterScheme::default()
    }])
}

/// `RagfairOfferGenerator.GetFleaPricesAsArray` (`:933-954`), minus its cache.
///
/// Legacy stores this list in `AllowedFleaPriceItemsForBarter` (`:56`) on first use and **never
/// invalidates it**, so a mod that edits prices or the barter blacklists after the first barter
/// offer of a server's life keeps getting the stale list. This port re-derives per call: same
/// content on stock data, fresher after such an edit. Documented divergence.
fn get_flea_prices_as_array<'a>(ctx: &RagfairContext<'a>) -> Vec<TplWithFleaPrice<'a>> {
    let barter_config = &ctx.dynamic.barter;
    let item_type_blacklist: Vec<&str> = barter_config
        .item_type_blacklist
        .iter()
        .map(String::as_str)
        .collect();

    ctx.flea_prices
        .iter()
        // Only get prices for items that also exist in items.json
        .filter(|(tpl, _)| get_item(ctx.items, tpl).is_some())
        .filter(|(tpl, _)| !is_of_baseclasses(ctx.items, tpl, &item_type_blacklist))
        .filter(|(tpl, _)| !barter_config.item_tpl_blacklist.contains(*tpl))
        .map(|(tpl, price)| TplWithFleaPrice {
            tpl: tpl.as_str(),
            price: *price,
        })
        .collect()
}

/// `RagfairOfferGenerator.CreateCurrencyBarterScheme` (`:963-969`) — the currency is drawn first,
/// then the price in that currency. The C# `multiplier` defaults to `1`; Rust has no default
/// arguments, so every call site passes it.
///
/// # Errors
///
/// Propagates [`get_dynamic_offer_currency`]'s and [`get_dynamic_offer_price_for_offer`]'s.
pub fn create_currency_barter_scheme(
    ctx: &mut RagfairContext,
    offer_with_children: &[Item],
    is_pack_offer: bool,
    multiplier: f64,
) -> Result<Vec<BarterScheme>, LootError> {
    let currency = get_dynamic_offer_currency(ctx)?;
    let price =
        get_dynamic_offer_price_for_offer(ctx, offer_with_children, &currency, is_pack_offer)?
            * multiplier;

    Ok(vec![BarterScheme {
        count: price,
        template: currency,
        ..BarterScheme::default()
    }])
}

/// `item.SlotId?.ToLowerInvariant()`, with the null case folded to the empty string — no slot id
/// is ever a member of the sets it is tested against.
fn lowercased_slot_id(item: &Item) -> String {
    item.slot_id.as_deref().unwrap_or_default().to_lowercase()
}

/// `GetRemovablePlateSlotIds().Contains(item.SlotId?.ToLowerInvariant())` (`:390`).
fn is_plate_slot(item: &Item) -> bool {
    get_removable_plate_slot_ids().contains(&lowercased_slot_id(item).as_str())
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use indexmap::{IndexMap, IndexSet};
    use serde_json::json;

    use super::*;
    use crate::loot::item_helper::{ARMOR, BUILT_IN_INSERTS};
    use crate::loot::models::{ItemView, PresetView, Upd};
    use crate::loot::random_util::TestSeedGuard;
    use crate::ragfair::models::{DynamicConfigWire, MinMaxIntWire};
    use crate::ragfair::{NO_BLACKLIST, NO_DEFAULT_PRESETS};

    const SEED: u64 = 20260813;

    const WEAPON_TPL: &str = "weapon_with_durability";
    const ARMOR_TPL: &str = "armor_vest_with_plate_slots";
    const ARMOR_NO_PLATE_SLOTS_TPL: &str = "armor_without_plate_slots";
    const PLATE_CLASS_4_TPL: &str = "class_4_plate";
    const PLATE_CLASS_6_TPL: &str = "class_6_plate";
    const SOFT_INSERT_TPL: &str = "class_3_soft_insert";
    const MEDKIT_TPL: &str = "medkit";
    const KEY_TPL: &str = "key";
    const FOOD_TPL: &str = "food";
    const REPAIR_KIT_TPL: &str = "repair_kit";
    const FUEL_TPL: &str = "fuel_can";
    const PLAIN_TPL: &str = "item_without_a_condition_entry";
    const REPAIRABLE_MEDKIT_TPL: &str = "repairable_and_medkit";
    const AMMO_BOX_TPL: &str = "ammo_box";
    const CARTRIDGE_TPL: &str = "cartridge";

    /// Offer roots, priced so each one lands on a different arm of the barter scheme.
    const BARTER_ROOT_TPL: &str = "barter_worthy_root";
    const CHEAP_ROOT_TPL: &str = "too_cheap_to_barter_root";
    const EXPENSIVE_ROOT_TPL: &str = "too_expensive_to_match_root";

    /// Barter candidates. Only the two `IN_RANGE` tpls can ever be picked for a
    /// [`BARTER_ROOT_TPL`] offer.
    const IN_RANGE_A_TPL: &str = "in_range_barter_item_a";
    const IN_RANGE_B_TPL: &str = "in_range_barter_item_b";
    const TOO_CHEAP_TPL: &str = "out_of_range_cheap_barter_item";
    const TOO_PRICEY_TPL: &str = "out_of_range_pricey_barter_item";
    const TPL_BLACKLISTED_TPL: &str = "in_range_but_tpl_blacklisted";
    const TYPE_BLACKLISTED_TPL: &str = "in_range_but_type_blacklisted";
    const BLACKLISTED_TYPE: &str = "blacklisted_base_class";
    /// Priced, in range, and absent from the items view — the `GetItem(...).Key` filter.
    const NOT_IN_ITEMS_VIEW_TPL: &str = "in_range_but_unknown_to_items_json";

    const USER_ID: &str = "offer_owner_id";
    const OFFER_TIME: i64 = 1_700_000_000;
    const END_TIME_MIN: f64 = 3600.0;
    const END_TIME_MAX: f64 = 7200.0;

    /// The condition entry every direct `randomise_item_condition` call uses: `max` is a non-empty
    /// range, so the `Max.Min` double-read is observable.
    const CONDITION_ID: &str = "condition_settings_base_class";
    const CONDITION_MAX_MIN: f64 = 0.5;
    const CONDITION_MAX_MAX: f64 = 0.9;
    const CONDITION_CURRENT_MIN: f64 = 0.4;
    const CONDITION_CURRENT_MAX: f64 = 0.8;

    struct Fixture {
        items: IndexMap<String, ItemView>,
        dynamic: DynamicConfigWire,
        prices: IndexMap<String, f64>,
        blacklist: HashSet<String>,
        presets: IndexMap<String, PresetView>,
        preset_lists: IndexMap<String, Vec<PresetView>>,
        names_usec: Vec<String>,
        names_bear: Vec<String>,
    }

    impl Fixture {
        fn new() -> Self {
            Self::with_condition_chance(1.0)
        }

        fn with_condition_chance(condition_chance: f64) -> Self {
            Self {
                items: serde_json::from_value(json!({
                    WEAPON_TPL: {"name": "weapon", "type": "Item", "parent": WEAPON,
                        "maxDurability": 100.0, "durability": 100.0},
                    ARMOR_TPL: {"name": "armor", "type": "Item", "parent": ARMOR, "armorClass": 0,
                        "durability": 50.0, "maxDurability": 50.0,
                        "slots": [{"name": "front_plate"}, {"name": "back_plate"},
                            {"name": "left_side_plate"}, {"name": "soft_armor_front"}]},
                    ARMOR_NO_PLATE_SLOTS_TPL: {"name": "plateless armor", "type": "Item",
                        "parent": ARMOR, "armorClass": 0,
                        "slots": [{"name": "soft_armor_front"}]},
                    PLATE_CLASS_4_TPL: {"name": "class 4 plate", "type": "Item",
                        "parent": ARMOR_PLATE, "armorClass": 4, "maxDurability": 40.0},
                    PLATE_CLASS_6_TPL: {"name": "class 6 plate", "type": "Item",
                        "parent": ARMOR_PLATE, "armorClass": 6, "maxDurability": 60.0},
                    SOFT_INSERT_TPL: {"name": "soft insert", "type": "Item",
                        "parent": BUILT_IN_INSERTS, "armorClass": 3, "maxDurability": 30.0},
                    MEDKIT_TPL: {"name": "medkit", "type": "Item", "maxHpResource": 400},
                    KEY_TPL: {"name": "key", "type": "Item", "maximumNumberOfUsage": 10},
                    FOOD_TPL: {"name": "food", "type": "Item", "maxResource": 100,
                        "foodUseTime": 5.0},
                    REPAIR_KIT_TPL: {"name": "repair kit", "type": "Item",
                        "maxRepairResource": 60.0},
                    FUEL_TPL: {"name": "fuel", "type": "Item", "parent": FUEL, "maxResource": 100},
                    PLAIN_TPL: {"name": "plain", "type": "Item"},
                    REPAIRABLE_MEDKIT_TPL: {"name": "two arms", "type": "Item",
                        "durability": 50.0, "maxHpResource": 400},
                    // The four `canSellOnRagfair` tpls are the whole sellable set of a full batch
                    // pass: every other priced tpl is filtered by the BSG-list arm of
                    // `is_item_valid_ragfair_item`.
                    AMMO_BOX_TPL: {"name": "ammo box", "type": "Item", "parent": AMMO_BOX,
                        "stackSlotMaxCount": 30.0, "stackSlotFirstFilterFirst": CARTRIDGE_TPL,
                        "canSellOnRagfair": true},
                    CARTRIDGE_TPL: {"name": "cartridge", "type": "Item", "stackMaxSize": 30},
                    BARTER_ROOT_TPL: {"name": "barter root", "type": "Item"},
                    CHEAP_ROOT_TPL: {"name": "cheap root", "type": "Item",
                        "canSellOnRagfair": true},
                    EXPENSIVE_ROOT_TPL: {"name": "expensive root", "type": "Item"},
                    IN_RANGE_A_TPL: {"name": "in range a", "type": "Item",
                        "canSellOnRagfair": true},
                    IN_RANGE_B_TPL: {"name": "in range b", "type": "Item"},
                    TOO_CHEAP_TPL: {"name": "too cheap", "type": "Item",
                        "canSellOnRagfair": true},
                    TOO_PRICEY_TPL: {"name": "too pricey", "type": "Item"},
                    TPL_BLACKLISTED_TPL: {"name": "tpl blacklisted", "type": "Item"},
                    TYPE_BLACKLISTED_TPL: {"name": "type blacklisted", "type": "Item",
                        "parent": BLACKLISTED_TYPE},
                    BLACKLISTED_TYPE: {"name": "blacklisted type base", "type": "Node"},
                    AMMO_BOX: {"name": "ammo box base", "type": "Node"},
                    // The base classes themselves, so the parent walk has somewhere to land.
                    WEAPON: {"name": "weapon base", "type": "Node"},
                    ARMOR: {"name": "armor base", "type": "Node"},
                    ARMOR_PLATE: {"name": "plate base", "type": "Node",
                        "parent": ARMORED_EQUIPMENT},
                    ARMORED_EQUIPMENT: {"name": "armored equipment base", "type": "Node"},
                    BUILT_IN_INSERTS: {"name": "soft insert base", "type": "Node"},
                    FUEL: {"name": "fuel base", "type": "Node"},
                }))
                .expect("items view parses"),
                dynamic: dynamic_config(condition_chance),
                // One map behind `flea_prices`, `handbook_prices` and `highest_trader_prices`, the
                // way the other ragfair fixtures wire it. Insertion order is the order
                // `get_flea_prices_as_array` hands its candidates to the index draw.
                prices: serde_json::from_value(json!({
                    BARTER_ROOT_TPL: 30_000.0,
                    CHEAP_ROOT_TPL: 100.0,
                    EXPENSIVE_ROOT_TPL: 1_000_000.0,
                    TOO_CHEAP_TPL: 100.0,
                    IN_RANGE_A_TPL: 15_000.5,
                    TPL_BLACKLISTED_TPL: 15_100.0,
                    TYPE_BLACKLISTED_TPL: 15_200.0,
                    NOT_IN_ITEMS_VIEW_TPL: 15_300.0,
                    IN_RANGE_B_TPL: 16_000.0,
                    TOO_PRICEY_TPL: 90_000.0,
                    AMMO_BOX_TPL: 5_000.4,
                    EUROS: 120.5,
                    DOLLARS: 130.0,
                }))
                .expect("price map parses"),
                blacklist: HashSet::new(),
                presets: IndexMap::new(),
                preset_lists: IndexMap::new(),
                names_usec: vec!["usec_alpha".to_owned(), "usec_beta".to_owned()],
                names_bear: vec!["bear_gamma".to_owned(), "bear_delta".to_owned()],
            }
        }

        fn ctx(&self) -> RagfairContext<'_> {
            RagfairContext {
                items: &self.items,
                dynamic: &self.dynamic,
                item_presets: &self.presets,
                default_presets: &NO_DEFAULT_PRESETS,
                default_presets_by_tpl: &self.presets,
                presets_by_tpl: &self.preset_lists,
                flea_prices: &self.prices,
                handbook_prices: &self.prices,
                highest_trader_prices: &self.prices,
                config_blacklist: &self.blacklist,
                seasonal_item_tpl_blacklist: &NO_BLACKLIST,
                pmc_names_usec: &self.names_usec,
                pmc_names_bear: &self.names_bear,
                timestamp: OFFER_TIME,
                seasonal_event_active: false,
                diagnostics: Vec::new(),
            }
        }
    }

    /// `armorPlate` blacklist: class 5+ banned, `back_plate` exempt.
    /// `armor`: plates always removed, and only `front_plate`/`left_side_plate` are in the pool.
    ///
    /// `barter` lists two items per offer at a 20% variance, so a [`BARTER_ROOT_TPL`] offer looks
    /// for candidates around half its price. `priceRanges` is deliberately *not* degenerate: a
    /// `min == max` range short-circuits [`get_biased_random_number`] without drawing, which would
    /// hide every price draw the barter arms are supposed to spend.
    fn dynamic_config(condition_chance: f64) -> DynamicConfigWire {
        serde_json::from_value(json!({
            "useTraderPriceForOffersIfHigher": false,
            "barter": {"chancePercent": 0.0, "itemCountMin": 2, "itemCountMax": 2,
                "priceRangeVariancePercent": 20.0, "minRoubleCostToBecomeBarter": 15_000.0,
                "makeSingleStackOnly": false, "itemTplBlacklist": [TPL_BLACKLISTED_TPL],
                "itemTypeBlacklist": [BLACKLISTED_TYPE]},
            "pack": {"chancePercent": 0.0, "itemCountMin": 1, "itemCountMax": 1,
                "itemTypeWhitelist": []},
            "offerAdjustment": {"adjustPriceWhenBelowHandbookPrice": false,
                "maxPriceDifferenceBelowHandbookPercent": 40.0, "handbookPriceMultiplier": 1.5,
                "priceThresholdRub": 6000.0},
            "offerItemCount": {"default": {"min": 1, "max": 1}},
            "priceRanges": {"default": {"min": 1.0, "max": 1.2},
                "preset": {"min": 1.0, "max": 1.2}, "pack": {"min": 1.0, "max": 1.2}},
            "showDefaultPresetsOnly": false,
            "ignoreQualityPriceVarianceBlacklist": [],
            "endTimeSeconds": {"min": 3600, "max": 7200},
            // Insertion order is the match order: ARMORED_EQUIPMENT is a *grandparent* of a plate
            // and still wins over the plate's direct parent because it is listed first.
            "condition": {
                ARMORED_EQUIPMENT: {"conditionChance": condition_chance,
                    "current": {"min": 0.1, "max": 0.2}, "max": {"min": 0.3, "max": 0.4}},
                ARMOR_PLATE: {"conditionChance": condition_chance,
                    "current": {"min": 0.1, "max": 0.2}, "max": {"min": 0.3, "max": 0.4}},
                WEAPON: {"conditionChance": condition_chance,
                    "current": {"min": CONDITION_CURRENT_MIN, "max": CONDITION_CURRENT_MAX},
                    "max": {"min": CONDITION_MAX_MIN, "max": CONDITION_MAX_MAX}},
                ARMOR: {"conditionChance": condition_chance,
                    "current": {"min": CONDITION_CURRENT_MIN, "max": CONDITION_CURRENT_MAX},
                    "max": {"min": CONDITION_MAX_MIN, "max": CONDITION_MAX_MAX}},
                CONDITION_ID: {"conditionChance": condition_chance,
                    "current": {"min": CONDITION_CURRENT_MIN, "max": CONDITION_CURRENT_MAX},
                    "max": {"min": CONDITION_MAX_MIN, "max": CONDITION_MAX_MAX}},
            },
            "stackablePercent": {"min": 10.0, "max": 100.0},
            "nonStackableCount": {"min": 1, "max": 4},
            "rating": {"min": 0.0, "max": 1.0},
            "armor": {"removeRemovablePlateChance": 100,
                "plateSlotIdToRemovePool": ["front_plate", "left_side_plate"]},
            "itemPriceMultiplier": {},
            // Three currencies whose weights do not sum to the entry count, so the weighted draw
            // takes its `get_double` arm rather than either free shortcut.
            "offerCurrencyChancePercent": {ROUBLES: 60.0, EUROS: 25.0, DOLLARS: 15.0},
            "showAsSingleStack": [],
            "removeSeasonalItemsWhenNotInEvent": false,
            "blacklist": {"damagedAmmoPacks": true, "custom": [], "enableBsgList": true,
                "enableQuestList": true, "traderItems": false,
                "armorPlate": {"maxProtectionLevel": 4, "ignoreSlots": ["back_plate"]},
                "enableCustomItemCategoryList": false, "customItemCategoryList": []},
            "unreasonableModPrices": {},
            "generateBaseFleaPrices": {"useHandbookPrice": false, "priceMultiplier": 1.0,
                "preventPriceBeingBelowTraderBuyPrice": false, "itemTplMultiplierOverride": {},
                "itemTypeMultiplierOverride": {}, "useHideoutCraftMultiplier": false,
                "hideoutCraftMultiplier": 1.0, "generatePresetPriceByChildren": false},
        }))
        .expect("dynamic config parses")
    }

    fn item(id: &str, tpl: &str) -> Item {
        Item {
            id: id.to_owned(),
            template: tpl.to_owned(),
            upd: Some(Upd::default()),
            ..Item::default()
        }
    }

    fn child(id: &str, tpl: &str, parent_id: &str, slot_id: &str) -> Item {
        Item {
            parent_id: Some(parent_id.to_owned()),
            slot_id: Some(slot_id.to_owned()),
            ..item(id, tpl)
        }
    }

    /// An armor root plus a class 6 front plate, a class 6 back plate, a class 4 left side plate
    /// and a class 3 soft insert.
    fn armor_with_plates() -> Vec<Item> {
        vec![
            item("armor_root", ARMOR_TPL),
            child("front", PLATE_CLASS_6_TPL, "armor_root", "front_plate"),
            child("back", PLATE_CLASS_6_TPL, "armor_root", "back_plate"),
            child("left", PLATE_CLASS_4_TPL, "armor_root", "left_side_plate"),
            child("soft", SOFT_INSERT_TPL, "armor_root", "soft_armor_front"),
        ]
    }

    /// Where the seeded stream stands after `consume` — the read-the-next-draw idiom the rest of
    /// the ragfair port pins draw counts with.
    fn stream_position_after(consume: impl FnOnce()) -> f64 {
        let _guard = TestSeedGuard::install(SEED);
        consume();

        get_double(0.0, 1.0)
    }

    /// The stream untouched, i.e. what a zero-draw arm has to leave behind.
    fn untouched_stream() -> f64 {
        stream_position_after(|| {})
    }

    /// The stream after the two `:699-700` multiplier draws plus `extra` further doubles.
    fn stream_position_after_condition_draws(extra: usize) -> f64 {
        stream_position_after(|| {
            get_double(CONDITION_MAX_MIN, CONDITION_MAX_MIN);
            get_double(CONDITION_CURRENT_MIN, CONDITION_CURRENT_MAX);
            for _ in 0..extra {
                get_double(0.0, 1.0);
            }
        })
    }

    fn seeded<T>(run: impl FnOnce() -> T) -> T {
        let _guard = TestSeedGuard::install(SEED);
        run()
    }

    fn condition(fixture: &Fixture, items: &mut [Item], tpl: &str) {
        randomise_item_condition(&fixture.ctx(), CONDITION_ID, items, tpl)
            .expect("the condition id is in the config");
    }

    // -----------------------------------------------------------------------
    // randomise_item_condition — the `Max.Min` double-read
    // -----------------------------------------------------------------------

    #[test]
    fn the_max_multiplier_is_the_max_min_bound_read_twice_and_still_costs_a_draw() {
        let fixture = Fixture::new();
        let mut items = vec![item("medkit", MEDKIT_TPL)];
        items[0].upd.as_mut().unwrap().med_kit = Some(UpdMedKit {
            hp_resource: Some(400.0),
        });

        seeded(|| condition(&fixture, &mut items, MEDKIT_TPL));

        // 400 * 0.5, not 400 * anything in (0.5, 0.9] — the degenerate range can only return
        // `Max.Min`.
        assert_eq!(
            items[0]
                .upd
                .as_ref()
                .unwrap()
                .med_kit
                .as_ref()
                .unwrap()
                .hp_resource,
            Some(200.0)
        );
        // ...and the draw was spent anyway.
        let after = stream_position_after(|| {
            let mut items = vec![item("medkit", MEDKIT_TPL)];
            items[0].upd.as_mut().unwrap().med_kit = Some(UpdMedKit {
                hp_resource: Some(400.0),
            });
            condition(&fixture, &mut items, MEDKIT_TPL);
        });
        assert_ne!(after, untouched_stream());
        assert_eq!(after, stream_position_after_condition_draws(0));
    }

    // -----------------------------------------------------------------------
    // randomise_item_condition — one test per branch, draw count pinned
    // -----------------------------------------------------------------------

    #[test]
    fn the_armor_branch_draws_four_times_per_child_above_armor_class_one() {
        let fixture = Fixture::new();
        let mut items = armor_with_plates();

        seeded(|| condition(&fixture, &mut items, ARMOR_TPL));

        // Root is class 0, the three mods are classes 6, 6, 4 and 3 - all above 1.
        assert!(items[0].upd.as_ref().unwrap().repairable.is_none());
        // Pinned to the seed, which pins the `(current, max)` parameter order this call site has
        // to pass them in: the first of each child's four draws is `GetDouble(maxMultiplier, 1)`,
        // so swapping the two arguments moves every number below.
        let durabilities: Vec<(f64, f64)> = items[1..]
            .iter()
            .map(|plate| {
                let repairable = plate.upd.as_ref().unwrap().repairable.as_ref().unwrap();
                (
                    repairable.max_durability.unwrap(),
                    repairable.durability.unwrap(),
                )
            })
            .collect();
        // Base max durabilities are 60, 60, 40 and 30.
        assert_eq!(
            durabilities,
            [(45.0, 42.0), (59.0, 59.0), (36.0, 35.0), (29.0, 25.0)]
        );

        let after = stream_position_after(|| {
            let mut items = armor_with_plates();
            condition(&fixture, &mut items, ARMOR_TPL);
        });
        assert_eq!(after, stream_position_after_condition_draws(4 * 4));
    }

    #[test]
    fn the_armor_branch_draw_count_follows_the_child_list() {
        let fixture = Fixture::new();

        let after = stream_position_after(|| {
            let mut items = vec![item("armor_root", ARMOR_TPL)];
            condition(&fixture, &mut items, ARMOR_TPL);
        });

        // A lone class 0 root qualifies nothing, so only the two multiplier draws happen.
        assert_eq!(after, stream_position_after_condition_draws(0));
    }

    #[test]
    fn a_plate_root_takes_the_armor_branch_without_holding_mods() {
        let fixture = Fixture::new();
        let mut items = vec![item("plate", PLATE_CLASS_6_TPL)];

        seeded(|| condition(&fixture, &mut items, PLATE_CLASS_6_TPL));

        assert!(items[0].upd.as_ref().unwrap().repairable.is_some());
    }

    #[test]
    fn the_weapon_branch_takes_four_draws() {
        let fixture = Fixture::new();
        let mut items = vec![item("weapon", WEAPON_TPL)];
        items[0].upd.as_mut().unwrap().repairable = Some(UpdRepairable {
            durability: Some(100.0),
            max_durability: Some(100.0),
            ..UpdRepairable::default()
        });

        seeded(|| condition(&fixture, &mut items, WEAPON_TPL));

        let repairable = items[0].upd.as_ref().unwrap().repairable.as_ref().unwrap();
        let max = repairable.max_durability.unwrap();
        let current = repairable.durability.unwrap();
        // Pinned to the seed, which also pins which multiplier reaches which bound: `max` comes
        // off `GetDouble(maxMultiplier, 1) * 100` and `current` off `GetDouble(currentMultiplier,
        // 1) * max`, so swapping the two multipliers moves both numbers.
        assert_eq!((max, current), (76.0, 71.0));
        assert!((50.0..=100.0).contains(&max), "max was {max}");
        assert!(current <= max && current > 0.0, "current was {current}");
        // Rounded, both of them.
        assert_eq!(max, max.trunc());
        assert_eq!(current, current.trunc());

        let after = stream_position_after(|| {
            let mut items = vec![item("weapon", WEAPON_TPL)];
            condition(&fixture, &mut items, WEAPON_TPL);
        });
        assert_eq!(after, stream_position_after_condition_draws(4));
    }

    #[test]
    fn the_medkit_branch_takes_no_further_draw_and_never_lands_on_zero() {
        let fixture = Fixture::new();
        let mut items = vec![item("medkit", MEDKIT_TPL)];
        // A resource small enough that `round(1 * 0.5)` is 0 - which the arm lifts to 1.
        items[0].upd.as_mut().unwrap().med_kit = Some(UpdMedKit {
            hp_resource: Some(1.0),
        });

        seeded(|| condition(&fixture, &mut items, MEDKIT_TPL));

        assert_eq!(
            items[0]
                .upd
                .as_ref()
                .unwrap()
                .med_kit
                .as_ref()
                .unwrap()
                .hp_resource,
            Some(1.0)
        );
        let after = stream_position_after(|| {
            let mut items = vec![item("medkit", MEDKIT_TPL)];
            items[0].upd.as_mut().unwrap().med_kit = Some(UpdMedKit {
                hp_resource: Some(1.0),
            });
            condition(&fixture, &mut items, MEDKIT_TPL);
        });
        assert_eq!(after, stream_position_after_condition_draws(0));
    }

    #[test]
    fn the_key_branch_uses_one_minus_the_max_multiplier() {
        let fixture = Fixture::new();
        let mut items = vec![item("key", KEY_TPL)];
        items[0].upd.as_mut().unwrap().key = Some(UpdKey {
            number_of_usages: Some(0),
        });

        seeded(|| condition(&fixture, &mut items, KEY_TPL));

        // round(10 * (1 - 0.5))
        assert_eq!(
            items[0]
                .upd
                .as_ref()
                .unwrap()
                .key
                .as_ref()
                .unwrap()
                .number_of_usages,
            Some(5)
        );
        let after = stream_position_after(|| {
            let mut items = vec![item("key", KEY_TPL)];
            items[0].upd.as_mut().unwrap().key = Some(UpdKey {
                number_of_usages: Some(0),
            });
            condition(&fixture, &mut items, KEY_TPL);
        });
        assert_eq!(after, stream_position_after_condition_draws(0));
    }

    #[test]
    fn a_single_use_key_falls_through_its_arm() {
        let fixture = Fixture {
            items: serde_json::from_value(json!({
                KEY_TPL: {"name": "single use key", "type": "Item", "maximumNumberOfUsage": 1},
            }))
            .expect("items view parses"),
            ..Fixture::new()
        };
        let mut items = vec![item("key", KEY_TPL)];
        items[0].upd.as_mut().unwrap().key = Some(UpdKey {
            number_of_usages: Some(0),
        });

        seeded(|| condition(&fixture, &mut items, KEY_TPL));

        // `MaximumNumberOfUsage > 1` gates the arm, so the value is untouched.
        assert_eq!(
            items[0]
                .upd
                .as_ref()
                .unwrap()
                .key
                .as_ref()
                .unwrap()
                .number_of_usages,
            Some(0)
        );
    }

    #[test]
    fn the_food_branch_reads_max_resource_from_the_template() {
        let fixture = Fixture::new();
        let mut items = vec![item("food", FOOD_TPL)];
        items[0].upd.as_mut().unwrap().food_drink = Some(UpdFoodDrink {
            hp_percent: Some(100.0),
        });

        seeded(|| condition(&fixture, &mut items, FOOD_TPL));

        // round(100 * 0.5)
        assert_eq!(
            items[0]
                .upd
                .as_ref()
                .unwrap()
                .food_drink
                .as_ref()
                .unwrap()
                .hp_percent,
            Some(50.0)
        );
        let after = stream_position_after(|| {
            let mut items = vec![item("food", FOOD_TPL)];
            items[0].upd.as_mut().unwrap().food_drink = Some(UpdFoodDrink {
                hp_percent: Some(100.0),
            });
            condition(&fixture, &mut items, FOOD_TPL);
        });
        assert_eq!(after, stream_position_after_condition_draws(0));
    }

    #[test]
    fn the_repair_kit_branch_reads_max_repair_resource_from_the_template() {
        let fixture = Fixture::new();
        let mut items = vec![item("repair kit", REPAIR_KIT_TPL)];
        items[0].upd.as_mut().unwrap().repair_kit = Some(UpdRepairKit {
            resource: Some(60.0),
        });

        seeded(|| condition(&fixture, &mut items, REPAIR_KIT_TPL));

        // round(60 * 0.5)
        assert_eq!(
            items[0]
                .upd
                .as_ref()
                .unwrap()
                .repair_kit
                .as_ref()
                .unwrap()
                .resource,
            Some(30.0)
        );
        let after = stream_position_after(|| {
            let mut items = vec![item("repair kit", REPAIR_KIT_TPL)];
            items[0].upd.as_mut().unwrap().repair_kit = Some(UpdRepairKit {
                resource: Some(60.0),
            });
            condition(&fixture, &mut items, REPAIR_KIT_TPL);
        });
        assert_eq!(after, stream_position_after_condition_draws(0));
    }

    #[test]
    fn the_fuel_branch_takes_one_further_draw_and_splits_the_capacity() {
        let fixture = Fixture::new();
        let mut items = vec![item("fuel", FUEL_TPL)];

        seeded(|| condition(&fixture, &mut items, FUEL_TPL));

        let resource = items[0].upd.as_ref().unwrap().resource.as_ref().unwrap();
        let remaining = resource.value.unwrap();
        assert!((50.0..=100.0).contains(&remaining), "value was {remaining}");
        assert_eq!(resource.units_consumed, Some(100.0 - remaining));

        let after = stream_position_after(|| {
            let mut items = vec![item("fuel", FUEL_TPL)];
            condition(&fixture, &mut items, FUEL_TPL);
        });
        assert_eq!(after, stream_position_after_condition_draws(1));
    }

    #[test]
    fn an_item_matching_no_branch_only_spends_the_two_multiplier_draws() {
        let fixture = Fixture::new();

        let after = stream_position_after(|| {
            let mut items = vec![item("plain", PLAIN_TPL)];
            condition(&fixture, &mut items, PLAIN_TPL);
        });

        assert_eq!(after, stream_position_after_condition_draws(0));
    }

    #[test]
    fn an_unknown_condition_id_is_an_error_and_costs_no_draw() {
        let fixture = Fixture::new();
        let mut items = vec![item("plain", PLAIN_TPL)];

        let error = randomise_item_condition(&fixture.ctx(), "nope", &mut items, PLAIN_TPL)
            .expect_err("an unknown condition id errors");

        assert_eq!(
            error.message,
            "The given key 'nope' was not present in the dictionary."
        );
        let after = stream_position_after(|| {
            let mut items = vec![item("plain", PLAIN_TPL)];
            randomise_item_condition(&fixture.ctx(), "nope", &mut items, PLAIN_TPL).unwrap_err();
        });
        assert_eq!(after, untouched_stream());
    }

    // -----------------------------------------------------------------------
    // The dead visor branch
    // -----------------------------------------------------------------------

    #[test]
    fn the_visor_branch_never_fires_on_realistic_data() {
        let fixture = Fixture::new();
        // A child in the visor slot, parented the way real data parents it: to the root item's id.
        let mut items = vec![
            item("armor_root", ARMOR_TPL),
            child("visor", PLAIN_TPL, "armor_root", "mod_equipment_000"),
        ];

        seeded(|| condition(&fixture, &mut items, ARMOR_TPL));

        assert!(
            !items[1]
                .upd
                .as_ref()
                .unwrap()
                .extra
                .contains_key("FaceShield")
        );
        let after = stream_position_after(|| {
            let mut items = vec![
                item("armor_root", ARMOR_TPL),
                child("visor", PLAIN_TPL, "armor_root", "mod_equipment_000"),
            ];
            condition(&fixture, &mut items, ARMOR_TPL);
        });
        // Neither the 25% roll nor the hit count was drawn.
        assert_eq!(after, stream_position_after_condition_draws(0));
    }

    #[test]
    fn the_visor_branch_fires_only_when_a_parent_id_is_a_base_class_tpl() {
        let fixture = Fixture::new();
        // Only reachable by parenting the child to the ARMORED_EQUIPMENT *base class tpl*, which
        // is what `:712` compares against and what no real item's `parentId` ever holds.
        let mut items = vec![
            item("armor_root", ARMOR_TPL),
            child("visor", PLAIN_TPL, ARMORED_EQUIPMENT, "mod_equipment_000"),
        ];

        let chance_passed = seeded(|| {
            get_double(CONDITION_MAX_MIN, CONDITION_MAX_MIN);
            get_double(CONDITION_CURRENT_MIN, CONDITION_CURRENT_MAX);

            get_chance_100(25.0)
        });
        seeded(|| condition(&fixture, &mut items, ARMOR_TPL));

        let face_shield = items[1].upd.as_ref().unwrap().extra.get("FaceShield");
        assert_eq!(face_shield.is_some(), chance_passed);
        if let Some(face_shield) = face_shield {
            let hits = face_shield["Hits"].as_i64().expect("Hits is a number");
            assert!((1..=3).contains(&hits), "hits was {hits}");
        }
    }

    // -----------------------------------------------------------------------
    // randomise_offer_item_upd_properties
    // -----------------------------------------------------------------------

    #[test]
    fn a_non_fake_player_offer_adds_conditions_but_never_randomises_them() {
        let fixture = Fixture::new();

        for creator in [OfferCreator::Player, OfferCreator::Trader] {
            let mut items = vec![item("weapon", WEAPON_TPL)];

            seeded(|| {
                randomise_offer_item_upd_properties(
                    &fixture.ctx(),
                    &mut items,
                    WEAPON_TPL,
                    creator,
                )
                .expect("the weapon template is in the view");
            });

            // AddMissingConditions still ran...
            let repairable = items[0].upd.as_ref().unwrap().repairable.as_ref().unwrap();
            assert_eq!(repairable.durability, Some(100.0));
            assert_eq!(repairable.max_durability, Some(100.0));

            // ...and nothing else did.
            let after = stream_position_after(|| {
                let mut items = vec![item("weapon", WEAPON_TPL)];
                randomise_offer_item_upd_properties(
                    &fixture.ctx(),
                    &mut items,
                    WEAPON_TPL,
                    creator,
                )
                .unwrap();
            });
            assert_eq!(after, untouched_stream());
        }
    }

    #[test]
    fn a_tpl_with_no_condition_entry_costs_no_draw() {
        let fixture = Fixture::new();

        let after = stream_position_after(|| {
            let mut items = vec![item("plain", PLAIN_TPL)];
            randomise_offer_item_upd_properties(
                &fixture.ctx(),
                &mut items,
                PLAIN_TPL,
                OfferCreator::FakePlayer,
            )
            .unwrap();
        });

        assert_eq!(after, untouched_stream());
    }

    #[test]
    fn a_failed_condition_chance_stops_after_its_single_draw() {
        let fixture = Fixture::with_condition_chance(0.0);

        let after = stream_position_after(|| {
            let mut items = vec![item("weapon", WEAPON_TPL)];
            randomise_offer_item_upd_properties(
                &fixture.ctx(),
                &mut items,
                WEAPON_TPL,
                OfferCreator::FakePlayer,
            )
            .unwrap();
        });

        assert_eq!(
            after,
            stream_position_after(|| {
                get_chance_100(0.0);
            })
        );
    }

    #[test]
    fn a_passed_condition_chance_randomises_the_condition() {
        let fixture = Fixture::new();
        let mut items = vec![item("weapon", WEAPON_TPL)];

        seeded(|| {
            randomise_offer_item_upd_properties(
                &fixture.ctx(),
                &mut items,
                WEAPON_TPL,
                OfferCreator::FakePlayer,
            )
            .expect("the weapon template is in the view");
        });

        // The weapon arm rewrote what AddMissingConditions seeded at 100/100.
        let repairable = items[0].upd.as_ref().unwrap().repairable.as_ref().unwrap();
        assert!(repairable.max_durability.unwrap() < 100.0);

        let after = stream_position_after(|| {
            let mut items = vec![item("weapon", WEAPON_TPL)];
            randomise_offer_item_upd_properties(
                &fixture.ctx(),
                &mut items,
                WEAPON_TPL,
                OfferCreator::FakePlayer,
            )
            .unwrap();
        });
        assert_eq!(
            after,
            stream_position_after(|| {
                get_chance_100(100.0);
                get_double(CONDITION_MAX_MIN, CONDITION_MAX_MIN);
                get_double(CONDITION_CURRENT_MIN, CONDITION_CURRENT_MAX);
                for _ in 0..4 {
                    get_double(0.0, 1.0);
                }
            })
        );
    }

    #[test]
    fn an_unknown_root_template_is_an_error_from_add_missing_conditions() {
        let fixture = Fixture::new();
        let mut items = vec![item("mystery", "no_such_tpl")];

        let error = randomise_offer_item_upd_properties(
            &fixture.ctx(),
            &mut items,
            "no_such_tpl",
            OfferCreator::FakePlayer,
        )
        .expect_err("an unknown tpl errors");

        assert_eq!(
            error.message,
            "Object reference not set to an instance of an object."
        );
    }

    // -----------------------------------------------------------------------
    // get_dynamic_condition_id_for_tpl
    // -----------------------------------------------------------------------

    #[test]
    fn the_first_matching_condition_key_in_insertion_order_wins() {
        let fixture = Fixture::new();

        // The plate's direct parent is ARMOR_PLATE, but ARMORED_EQUIPMENT - its grandparent - is
        // listed first in the config.
        assert_eq!(
            get_dynamic_condition_id_for_tpl(&fixture.ctx(), PLATE_CLASS_6_TPL),
            Some(ARMORED_EQUIPMENT.to_owned())
        );
        assert_eq!(
            get_dynamic_condition_id_for_tpl(&fixture.ctx(), WEAPON_TPL),
            Some(WEAPON.to_owned())
        );
        assert_eq!(
            get_dynamic_condition_id_for_tpl(&fixture.ctx(), PLAIN_TPL),
            None
        );
    }

    // -----------------------------------------------------------------------
    // add_missing_conditions
    // -----------------------------------------------------------------------

    #[test]
    fn the_first_matching_condition_arm_wins_and_returns() {
        let fixture = Fixture::new();
        let mut repairable_medkit = item("both", REPAIRABLE_MEDKIT_TPL);

        add_missing_conditions(&fixture.ctx(), &mut repairable_medkit).unwrap();

        let upd = repairable_medkit.upd.as_ref().unwrap();
        assert_eq!(upd.repairable.as_ref().unwrap().durability, Some(50.0));
        assert_eq!(upd.repairable.as_ref().unwrap().max_durability, Some(50.0));
        assert!(upd.med_kit.is_none(), "the medkit arm must not also fire");
    }

    #[test]
    fn every_condition_arm_writes_its_own_upd_member_without_drawing() {
        let fixture = Fixture::new();

        /// "Did this arm write the member it is supposed to write?"
        type ArmCheck = fn(&Upd) -> bool;

        let cases: [(&str, ArmCheck); 5] = [
            (WEAPON_TPL, |upd| {
                upd.repairable.as_ref().is_some_and(|repairable| {
                    repairable.durability == Some(100.0) && repairable.max_durability == Some(100.0)
                })
            }),
            (MEDKIT_TPL, |upd| {
                upd.med_kit
                    .as_ref()
                    .is_some_and(|med_kit| med_kit.hp_resource == Some(400.0))
            }),
            (KEY_TPL, |upd| {
                upd.key
                    .as_ref()
                    .is_some_and(|key| key.number_of_usages == Some(0))
            }),
            (FOOD_TPL, |upd| {
                upd.food_drink
                    .as_ref()
                    .is_some_and(|food| food.hp_percent == Some(100.0))
            }),
            (REPAIR_KIT_TPL, |upd| {
                upd.repair_kit
                    .as_ref()
                    .is_some_and(|kit| kit.resource == Some(60.0))
            }),
        ];

        for (tpl, check) in cases {
            let mut subject = item("subject", tpl);
            add_missing_conditions(&fixture.ctx(), &mut subject).unwrap();
            assert!(
                check(subject.upd.as_ref().unwrap()),
                "{tpl} was not written"
            );

            let after = stream_position_after(|| {
                let mut subject = item("subject", tpl);
                add_missing_conditions(&fixture.ctx(), &mut subject).unwrap();
            });
            assert_eq!(after, untouched_stream(), "{tpl} drew from the stream");
        }
    }

    #[test]
    fn an_item_with_no_condition_properties_is_left_alone() {
        let fixture = Fixture::new();
        let mut plain = item("plain", PLAIN_TPL);

        add_missing_conditions(&fixture.ctx(), &mut plain).unwrap();

        let upd = plain.upd.as_ref().unwrap();
        assert!(upd.repairable.is_none());
        assert!(upd.med_kit.is_none());
        assert!(upd.key.is_none());
        assert!(upd.food_drink.is_none());
        assert!(upd.repair_kit.is_none());
    }

    // -----------------------------------------------------------------------
    // remove_banned_plates_from_preset
    // -----------------------------------------------------------------------

    #[test]
    fn only_over_level_non_ignored_plates_are_removed() {
        let fixture = Fixture::new();
        let mut preset = armor_with_plates();

        let removed = remove_banned_plates_from_preset(
            &fixture.ctx(),
            &mut preset,
            &fixture.dynamic.blacklist.armor_plate,
        );

        assert!(removed);
        // The class 6 front plate went; the ignored class 6 back plate, the class 4 side plate
        // (4 is not > 4) and the soft insert stayed, in their original order.
        let ids: Vec<&str> = preset.iter().map(|item| item.id.as_str()).collect();
        assert_eq!(ids, ["armor_root", "back", "left", "soft"]);
    }

    #[test]
    fn removal_survives_the_index_shift_it_causes() {
        let fixture = Fixture::new();
        // Two removable plates, so the second one's index moves when the first is removed.
        let mut preset = vec![
            item("armor_root", ARMOR_TPL),
            child("front", PLATE_CLASS_6_TPL, "armor_root", "front_plate"),
            child("left", PLATE_CLASS_6_TPL, "armor_root", "left_side_plate"),
            child("soft", SOFT_INSERT_TPL, "armor_root", "soft_armor_front"),
        ];

        let removed = remove_banned_plates_from_preset(
            &fixture.ctx(),
            &mut preset,
            &fixture.dynamic.blacklist.armor_plate,
        );

        assert!(removed);
        let ids: Vec<&str> = preset.iter().map(|item| item.id.as_str()).collect();
        assert_eq!(ids, ["armor_root", "soft"]);
    }

    #[test]
    fn a_preset_that_cannot_hold_mods_or_has_no_plate_slots_is_untouched() {
        let fixture = Fixture::new();

        let mut weapon = vec![item("weapon", WEAPON_TPL)];
        assert!(!remove_banned_plates_from_preset(
            &fixture.ctx(),
            &mut weapon,
            &fixture.dynamic.blacklist.armor_plate
        ));
        assert_eq!(weapon.len(), 1);

        let mut soft_only = vec![
            item("armor_root", ARMOR_TPL),
            child("soft", SOFT_INSERT_TPL, "armor_root", "soft_armor_front"),
        ];
        assert!(!remove_banned_plates_from_preset(
            &fixture.ctx(),
            &mut soft_only,
            &fixture.dynamic.blacklist.armor_plate
        ));
        assert_eq!(soft_only.len(), 2);
    }

    #[test]
    fn removing_banned_plates_costs_no_draw() {
        let fixture = Fixture::new();

        let after = stream_position_after(|| {
            let mut preset = armor_with_plates();
            remove_banned_plates_from_preset(
                &fixture.ctx(),
                &mut preset,
                &fixture.dynamic.blacklist.armor_plate,
            );
        });

        assert_eq!(after, untouched_stream());
    }

    // -----------------------------------------------------------------------
    // remove_armor_plates
    // -----------------------------------------------------------------------

    #[test]
    fn an_armor_with_no_removable_plate_slots_still_spends_the_chance_draw() {
        let fixture = Fixture::new();
        let mut items = vec![
            item("armor_root", ARMOR_NO_PLATE_SLOTS_TPL),
            child("soft", SOFT_INSERT_TPL, "armor_root", "soft_armor_front"),
        ];

        seeded(|| remove_armor_plates(&fixture.ctx(), &mut items));

        assert_eq!(items.len(), 2);
        let after = stream_position_after(|| {
            let mut items = vec![
                item("armor_root", ARMOR_NO_PLATE_SLOTS_TPL),
                child("soft", SOFT_INSERT_TPL, "armor_root", "soft_armor_front"),
            ];
            remove_armor_plates(&fixture.ctx(), &mut items);
        });
        assert_eq!(
            after,
            stream_position_after(|| {
                get_chance_100(100.0);
            })
        );
    }

    #[test]
    fn plates_in_the_removal_pool_are_removed_back_to_front() {
        let fixture = Fixture::new();
        let mut items = armor_with_plates();

        seeded(|| remove_armor_plates(&fixture.ctx(), &mut items));

        // The pool holds `front_plate` and `left_side_plate`; `back_plate` is not in it.
        let ids: Vec<&str> = items.iter().map(|item| item.id.as_str()).collect();
        assert_eq!(ids, ["armor_root", "back", "soft"]);
    }

    #[test]
    fn a_failed_chance_leaves_every_plate_in_place() {
        let fixture = Fixture {
            dynamic: dynamic_config_with_plate_chance(0),
            ..Fixture::new()
        };
        let mut items = armor_with_plates();

        seeded(|| remove_armor_plates(&fixture.ctx(), &mut items));

        assert_eq!(items.len(), 5);
    }

    fn dynamic_config_with_plate_chance(chance: i32) -> DynamicConfigWire {
        let mut dynamic = dynamic_config(1.0);
        dynamic.armor.remove_removable_plate_chance = chance;

        dynamic
    }

    // -----------------------------------------------------------------------
    // create_user_data_for_flea_offer
    // -----------------------------------------------------------------------

    #[test]
    fn the_fake_player_user_block_draws_faction_nickname_rating_growth_and_aid_in_that_order() {
        let fixture = Fixture::new();

        let (faction, nickname, rating, is_rating_growing, aid) = seeded(|| {
            let faction = get_int(0, 1);
            let pool = if faction == 0 {
                &fixture.names_usec
            } else {
                &fixture.names_bear
            };
            let nickname = get_array_value(pool).clone();

            (
                faction,
                nickname,
                get_double(0.0, 1.0),
                get_bool(),
                generate_account_id(),
            )
        });

        let user = seeded(|| create_user_data_for_flea_offer(&fixture.ctx(), USER_ID, false))
            .expect("both name pools are populated");

        assert_eq!(user.id, USER_ID);
        assert_eq!(user.member_type, 0);
        assert_eq!(user.nickname.as_deref(), Some(nickname.as_str()));
        // The nickname came out of the faction the *first* draw selected, so the two draws cannot
        // be swapped without breaking this.
        let drew_a_usec_name = fixture
            .names_usec
            .iter()
            .any(|name| Some(name.as_str()) == user.nickname.as_deref());
        assert_eq!(drew_a_usec_name, faction == 0);
        assert_eq!(user.rating, rating);
        assert_eq!(user.is_rating_growing, is_rating_growing);
        assert_eq!(user.aid, aid);
        assert!(user.avatar.is_none());
    }

    #[test]
    fn a_trader_offer_user_is_an_id_and_a_member_type_and_costs_no_draw() {
        let fixture = Fixture::new();

        let user = create_user_data_for_flea_offer(&fixture.ctx(), "trader_id", true)
            .expect("the trader arm cannot fail");

        assert_eq!(user.id, "trader_id");
        // MemberCategory.Trader
        assert_eq!(user.member_type, 4);
        assert_eq!(user.nickname, None);

        let after = stream_position_after(|| {
            create_user_data_for_flea_offer(&fixture.ctx(), "trader_id", true).unwrap();
        });
        assert_eq!(after, untouched_stream());
    }

    #[test]
    fn an_empty_name_pool_errors_after_the_faction_draw_is_already_spent() {
        let fixture = Fixture {
            names_usec: Vec::new(),
            names_bear: Vec::new(),
            ..Fixture::new()
        };

        let error = seeded(|| create_user_data_for_flea_offer(&fixture.ctx(), USER_ID, false))
            .expect_err("an empty name pool is a C# throw");

        assert!(error.message.contains("empty"), "{}", error.message);
        let after = stream_position_after(|| {
            create_user_data_for_flea_offer(&fixture.ctx(), USER_ID, false).unwrap_err();
        });
        assert_eq!(
            after,
            stream_position_after(|| {
                get_int(0, 1);
            })
        );
    }

    // -----------------------------------------------------------------------
    // get_flea_prices_as_array
    // -----------------------------------------------------------------------

    #[test]
    fn the_barter_candidate_list_keeps_flea_price_order_minus_both_blacklists() {
        let fixture = Fixture::new();

        let candidates = get_flea_prices_as_array(&fixture.ctx());

        let tpls: Vec<&str> = candidates.iter().map(|entry| entry.tpl).collect();
        assert_eq!(
            tpls,
            [
                BARTER_ROOT_TPL,
                CHEAP_ROOT_TPL,
                EXPENSIVE_ROOT_TPL,
                TOO_CHEAP_TPL,
                IN_RANGE_A_TPL,
                IN_RANGE_B_TPL,
                TOO_PRICEY_TPL,
                AMMO_BOX_TPL,
            ]
        );
        assert_eq!(candidates[4].price, 15_000.5);

        let after = stream_position_after(|| {
            get_flea_prices_as_array(&fixture.ctx());
        });
        assert_eq!(after, untouched_stream());
    }

    // -----------------------------------------------------------------------
    // create_currency_barter_scheme
    // -----------------------------------------------------------------------

    #[test]
    fn a_currency_scheme_draws_the_currency_before_the_price_and_applies_the_multiplier() {
        let fixture = Fixture::new();
        let items = vec![item("root", BARTER_ROOT_TPL)];

        let (currency, price) = seeded(|| {
            let currency = get_dynamic_offer_currency(&fixture.ctx()).unwrap();
            let price =
                get_dynamic_offer_price_for_offer(&mut fixture.ctx(), &items, &currency, true)
                    .unwrap();

            (currency, price)
        });
        let scheme =
            seeded(|| create_currency_barter_scheme(&mut fixture.ctx(), &items, true, 3.0))
                .expect("the currency map is populated");

        assert_eq!(scheme.len(), 1);
        assert_eq!(scheme[0].template, currency);
        assert_eq!(scheme[0].count, price * 3.0);
        assert!(scheme[0].only_functional.is_none());
        assert!(scheme[0].level.is_none());
    }

    // -----------------------------------------------------------------------
    // create_barter_barter_scheme
    // -----------------------------------------------------------------------

    #[test]
    fn a_barter_scheme_above_the_threshold_lists_an_in_range_flea_item() {
        let fixture = Fixture::new();
        let items = vec![item("root", BARTER_ROOT_TPL)];

        let scheme = seeded(|| {
            create_barter_barter_scheme(&mut fixture.ctx(), &items, &fixture.dynamic.barter)
        })
        .expect("the barter root is priced above the threshold");

        assert_eq!(scheme.len(), 1);
        // `itemCountMin` == `itemCountMax` == 2.
        assert_eq!(scheme[0].count, 2.0);
        // A 30k offer at two items is 15k each ± 20%, which only these two candidates fit; the
        // exact one is pinned to the seed.
        assert!(
            [IN_RANGE_A_TPL, IN_RANGE_B_TPL].contains(&scheme[0].template.as_str()),
            "picked {}",
            scheme[0].template
        );
        assert_eq!(scheme[0].template, IN_RANGE_A_TPL);
    }

    #[test]
    fn a_barter_scheme_below_the_threshold_falls_through_after_paying_for_a_price_draw() {
        let fixture = Fixture::new();
        let items = vec![item("root", CHEAP_ROOT_TPL)];

        let scheme = seeded(|| {
            create_barter_barter_scheme(&mut fixture.ctx(), &items, &fixture.dynamic.barter)
        })
        .expect("the currency fall-through cannot fail here");

        assert!(is_money_tpl(&scheme[0].template));

        let after = stream_position_after(|| {
            create_barter_barter_scheme(&mut fixture.ctx(), &items, &fixture.dynamic.barter)
                .unwrap();
        });
        // The rouble price draw the barter arm spends and throws away, then the currency scheme's
        // own two draws - the legacy double-draw.
        assert_eq!(
            after,
            stream_position_after(|| {
                get_dynamic_offer_price_for_offer(&mut fixture.ctx(), &items, ROUBLES, false)
                    .unwrap();
                create_currency_barter_scheme(&mut fixture.ctx(), &items, false, 1.0).unwrap();
            })
        );
        // ...and that discarded draw is exactly what a plain currency scheme does not spend.
        assert_ne!(
            after,
            stream_position_after(|| {
                create_currency_barter_scheme(&mut fixture.ctx(), &items, false, 1.0).unwrap();
            })
        );
    }

    #[test]
    fn an_empty_in_range_candidate_list_falls_through_to_a_third_price_draw() {
        let fixture = Fixture::new();
        // 1m roubles over two items wants 500k ± 20% candidates; the priciest on the flea is 90k.
        let items = vec![item("root", EXPENSIVE_ROOT_TPL)];

        let scheme = seeded(|| {
            create_barter_barter_scheme(&mut fixture.ctx(), &items, &fixture.dynamic.barter)
        })
        .expect("the currency fall-through cannot fail here");

        assert!(is_money_tpl(&scheme[0].template));

        let after = stream_position_after(|| {
            create_barter_barter_scheme(&mut fixture.ctx(), &items, &fixture.dynamic.barter)
                .unwrap();
        });
        assert_eq!(
            after,
            stream_position_after(|| {
                get_dynamic_offer_price_for_offer(&mut fixture.ctx(), &items, ROUBLES, false)
                    .unwrap();
                // The item count draw is degenerate here (2..=2) and costs nothing, but it is
                // still the second thing the arm does.
                get_int(2, 2);
                create_currency_barter_scheme(&mut fixture.ctx(), &items, false, 1.0).unwrap();
            })
        );
    }

    // -----------------------------------------------------------------------
    // convert_offer_requirements_into_roubles / calculate_rouble_price
    // -----------------------------------------------------------------------

    #[test]
    fn money_requirements_are_rounded_and_item_requirements_are_not() {
        let fixture = Fixture::new();
        let requirements = vec![requirement(EUROS, 1.5), requirement(IN_RANGE_A_TPL, 0.5)];

        let roubles = convert_offer_requirements_into_roubles(&fixture.ctx(), &requirements);

        // 1.5 euros at 120.5 roubles each is 180.75, rounded to 181; half of a 15000.5 rouble item
        // keeps its quarter.
        assert_eq!(roubles, 181.0 + 7500.25);
    }

    #[test]
    fn roubles_pass_through_and_other_currencies_go_via_the_handbook() {
        let fixture = Fixture::new();

        assert_eq!(
            calculate_rouble_price(&fixture.ctx(), 1.5, ROUBLES),
            1.5,
            "roubles are not converted, let alone rounded"
        );
        assert_eq!(calculate_rouble_price(&fixture.ctx(), 1.5, EUROS), 181.0);
        // A currency with no handbook entry prices at zero, the way GetTemplatePrice answers.
        assert_eq!(calculate_rouble_price(&fixture.ctx(), 1.5, GP), 0.0);
    }

    // -----------------------------------------------------------------------
    // get_offer_end_time
    // -----------------------------------------------------------------------

    #[test]
    fn a_fake_player_offer_ends_one_random_spread_after_its_start_time() {
        let fixture = Fixture::new();

        let expected =
            seeded(|| round_half_even(OFFER_TIME as f64 + get_double(END_TIME_MIN, END_TIME_MAX)));
        let end_time = seeded(|| {
            get_offer_end_time(
                &fixture.ctx(),
                OfferCreator::FakePlayer,
                USER_ID,
                OFFER_TIME,
            )
        })
        .expect("the fake-player arm cannot fail");

        assert_eq!(end_time, expected as i64);
        assert!(end_time > OFFER_TIME);
    }

    #[test]
    fn the_player_and_trader_arms_error_on_inputs_the_wire_does_not_carry() {
        let fixture = Fixture::new();

        for creator in [OfferCreator::Player, OfferCreator::Trader] {
            let error = get_offer_end_time(&fixture.ctx(), creator, USER_ID, OFFER_TIME)
                .expect_err("neither input crosses the FFI boundary");

            assert!(!error.message.is_empty());
        }
    }

    // -----------------------------------------------------------------------
    // create_offer
    // -----------------------------------------------------------------------

    fn requirement(tpl: &str, count: f64) -> OfferRequirementWire {
        OfferRequirementWire {
            template_id: tpl.to_owned(),
            count,
            only_functional: false,
            level: None,
            side: None,
        }
    }

    fn rouble_scheme(count: f64) -> Vec<BarterScheme> {
        vec![BarterScheme {
            count,
            template: ROUBLES.to_owned(),
            ..BarterScheme::default()
        }]
    }

    fn offer_details(items: Vec<Item>, barter_scheme: Vec<BarterScheme>) -> CreateFleaOfferDetails {
        CreateFleaOfferDetails {
            user_id: USER_ID.to_owned(),
            time: OFFER_TIME,
            items,
            barter_scheme,
            loyal_level: 1,
            quantity: 1,
            creator: OfferCreator::FakePlayer,
            sell_in_one_piece: false,
        }
    }

    #[test]
    fn a_lone_ammo_box_is_hydrated_with_cartridges() {
        let fixture = Fixture::new();
        let details = offer_details(vec![item("box", AMMO_BOX_TPL)], rouble_scheme(5_000.0));
        let mut offer_counter = 0;

        let offer = seeded(|| create_offer(&mut fixture.ctx(), &details, &mut offer_counter))
            .expect("the ammo box template is complete");

        assert_eq!(offer.items.len(), 2);
        assert_eq!(offer.items[1].template, CARTRIDGE_TPL);
        // The caller's list is untouched; the offer carries the hydrated copy.
        assert_eq!(details.items.len(), 1);
    }

    #[test]
    fn an_ammo_box_with_a_child_already_present_is_left_alone() {
        let fixture = Fixture::new();
        let details = offer_details(
            vec![
                item("box", AMMO_BOX_TPL),
                child("loose", PLAIN_TPL, "box", "cartridges"),
            ],
            rouble_scheme(5_000.0),
        );
        let mut offer_counter = 0;

        let offer = seeded(|| create_offer(&mut fixture.ctx(), &details, &mut offer_counter))
            .expect("the ammo box template is complete");

        // The hydration gate is `Count == 1`, not "is an ammo box".
        assert_eq!(offer.items.len(), 2);
    }

    #[test]
    fn each_offer_takes_the_next_internal_id_and_bumps_the_counter() {
        let fixture = Fixture::new();
        let details = offer_details(vec![item("root", BARTER_ROOT_TPL)], rouble_scheme(5_000.0));
        let mut offer_counter = 7;

        let (first, second) = seeded(|| {
            (
                create_offer(&mut fixture.ctx(), &details, &mut offer_counter).unwrap(),
                create_offer(&mut fixture.ctx(), &details, &mut offer_counter).unwrap(),
            )
        });

        assert_eq!((first.internal_id, second.internal_id), (7, 8));
        assert_eq!(offer_counter, 9);
        assert_ne!(first.id, second.id);
    }

    #[test]
    fn a_pack_offer_divides_its_listing_price_by_the_quantity() {
        let fixture = Fixture::new();
        let details = CreateFleaOfferDetails {
            quantity: 3,
            sell_in_one_piece: true,
            ..offer_details(vec![item("box", AMMO_BOX_TPL)], rouble_scheme(100.0))
        };
        let mut offer_counter = 0;

        let offer = seeded(|| create_offer(&mut fixture.ctx(), &details, &mut offer_counter))
            .expect("the ammo box template is complete");

        assert_eq!(offer.summary_cost, 100.0);
        // round(100 / 3)
        assert_eq!(offer.requirements_cost, 33.0);
        assert_eq!(offer.requirements[0].count, 100.0);
        // The handbook price of the root, rounded.
        assert_eq!(offer.items_cost, 5_000.0);
        assert_eq!(offer.root, "box");
        assert_eq!(offer.start_time, OFFER_TIME);
        assert_eq!(offer.loyalty_level, 1);
        assert!(offer.sell_in_one_piece);
        assert!(!offer.locked);
        assert_eq!(offer.quantity, 3);
    }

    #[test]
    fn requirement_counts_round_half_to_even_at_two_decimals() {
        let fixture = Fixture::new();
        let details = offer_details(vec![item("root", BARTER_ROOT_TPL)], rouble_scheme(0.125));
        let mut offer_counter = 0;

        let offer = seeded(|| create_offer(&mut fixture.ctx(), &details, &mut offer_counter))
            .expect("the barter root is in the view");

        // Banker's rounding: 0.125 goes down to 0.12, not up to 0.13.
        assert_eq!(offer.requirements[0].count, 0.12);
        assert!(!offer.requirements[0].only_functional);
    }

    #[test]
    fn level_and_side_only_ride_along_when_the_barter_sets_a_level() {
        let fixture = Fixture::new();
        let details = offer_details(
            vec![item("root", BARTER_ROOT_TPL)],
            vec![
                BarterScheme {
                    count: 1.0,
                    template: "dogtag".to_owned(),
                    only_functional: Some(true),
                    level: Some(5),
                    side: Some(1),
                },
                BarterScheme {
                    count: 1.0,
                    template: "dogtag".to_owned(),
                    only_functional: None,
                    level: None,
                    // Dropped: the C# only copies `Side` inside the `Level != null` branch.
                    side: Some(1),
                },
            ],
        );
        let mut offer_counter = 0;

        let offer = seeded(|| create_offer(&mut fixture.ctx(), &details, &mut offer_counter))
            .expect("the barter root is in the view");

        assert_eq!(offer.requirements[0].level, Some(5));
        assert_eq!(offer.requirements[0].side, Some(1));
        assert!(offer.requirements[0].only_functional);
        assert_eq!(offer.requirements[1].level, None);
        assert_eq!(offer.requirements[1].side, None);
        assert!(!offer.requirements[1].only_functional);
    }

    #[test]
    fn an_offer_spends_the_five_user_draws_then_the_end_time_draw() {
        let fixture = Fixture::new();
        let details = offer_details(vec![item("root", BARTER_ROOT_TPL)], rouble_scheme(5_000.0));
        let mut offer_counter = 0;

        let after = stream_position_after(|| {
            let mut counter = 0;
            create_offer(&mut fixture.ctx(), &details, &mut counter).unwrap();
        });

        assert_eq!(
            after,
            stream_position_after(|| {
                create_user_data_for_flea_offer(&fixture.ctx(), USER_ID, false).unwrap();
                get_offer_end_time(
                    &fixture.ctx(),
                    OfferCreator::FakePlayer,
                    USER_ID,
                    OFFER_TIME,
                )
                .unwrap();
            })
        );

        let offer = seeded(|| create_offer(&mut fixture.ctx(), &details, &mut offer_counter))
            .expect("the barter root is in the view");
        assert_eq!(offer.summary_cost, 5_000.0);
        assert_eq!(offer.requirements_cost, 5_000.0);
        assert_eq!(offer.items_cost, 30_000.0);
    }

    #[test]
    fn an_offer_with_no_items_is_an_error() {
        let fixture = Fixture::new();
        let details = offer_details(Vec::new(), rouble_scheme(5_000.0));
        let mut offer_counter = 0;

        let error = seeded(|| create_offer(&mut fixture.ctx(), &details, &mut offer_counter))
            .expect_err("the C# dereferences a null root item");

        assert_eq!(
            error.message,
            "Object reference not set to an instance of an object."
        );
        // The throw happens before the counter is bumped.
        assert_eq!(offer_counter, 0);
    }

    // -----------------------------------------------------------------------
    // generate_dynamic_offers — the batch pass
    // -----------------------------------------------------------------------

    /// The whole [`Fixture`] as one batch request; consumes it, since the request owns every view.
    ///
    /// `defaultPresetsByTpl`/`defaultPresets` stay empty: only the weapon-preset pricing arm reads
    /// them, and no fixture offer is a weapon preset.
    fn request(fixture: Fixture) -> GenerateDynamicOffersRequest {
        GenerateDynamicOffersRequest {
            test_seed: Some(SEED),
            timestamp: OFFER_TIME,
            offer_counter_start: 0,
            expired_offers: None,
            dynamic: fixture.dynamic,
            item_presets: fixture.presets,
            default_presets: Vec::new(),
            default_presets_by_tpl: IndexMap::new(),
            presets_by_tpl: fixture.preset_lists,
            flea_prices: fixture.prices.clone(),
            handbook_prices: fixture.prices.clone(),
            highest_trader_prices: fixture.prices,
            config_blacklist: fixture.blacklist.into_iter().collect(),
            seasonal_event_active: false,
            seasonal_item_tpl_blacklist: Vec::new(),
            pmc_names_usec: fixture.names_usec,
            pmc_names_bear: fixture.names_bear,
            items: fixture.items,
        }
    }

    /// `(root tpl, intId, quantity, sellInOnePiece, requirement tpl, requirement count)` — every
    /// offer field a batch pass can be pinned on. Ids are minted off a process-local counter and
    /// end times off the clock-independent seed, so they are normalised away here and asserted
    /// separately.
    fn normalised(offers: &[RagfairOfferWire]) -> Vec<(&str, i32, i32, bool, &str, f64)> {
        offers
            .iter()
            .map(|offer| {
                (
                    offer.items[0].template.as_str(),
                    offer.internal_id,
                    offer.quantity,
                    offer.sell_in_one_piece,
                    offer.requirements[0].template_id.as_str(),
                    offer.requirements[0].count,
                )
            })
            .collect()
    }

    /// A range that costs no draw, so a test can zero out one step of the draw sequence.
    fn fixed(value: i32) -> MinMaxIntWire {
        MinMaxIntWire {
            min: value,
            max: value,
        }
    }

    fn offers_of(fixture: Fixture) -> DynamicOffersResult {
        generate_dynamic_offers(request(fixture)).expect("the fixture is complete")
    }

    #[test]
    fn a_full_pass_makes_offers_for_the_sellable_templates_in_items_view_order() {
        let result = offers_of(Fixture::new());

        // The four `canSellOnRagfair` tpls, in items-view order — every other priced tpl is
        // filtered by the BSG-list arm. `offerItemCount.default` is `{1, 1}`, so one offer each.
        assert_eq!(
            result
                .offers
                .iter()
                .map(|offer| offer.items[0].template.as_str())
                .collect::<Vec<_>>(),
            [AMMO_BOX_TPL, CHEAP_ROOT_TPL, IN_RANGE_A_TPL, TOO_CHEAP_TPL]
        );
        assert!(result.rejected_can_sell_templates.is_empty());
        // The ammo box is hydrated inside `create_offer`, so the cartridge is on the offer.
        assert_eq!(result.offers[0].items.len(), 2);
        assert_eq!(result.offers[0].items[1].template, CARTRIDGE_TPL);
        for offer in &result.offers {
            assert_eq!(offer.start_time, OFFER_TIME);
            assert_eq!(offer.loyalty_level, 1);
            assert!(!offer.sell_in_one_piece);
            assert!(
                (1..=4).contains(&offer.quantity),
                "quantity was {}",
                offer.quantity
            );
            assert!(
                offer.end_time > OFFER_TIME + END_TIME_MIN as i64,
                "end time was {}",
                offer.end_time
            );
        }
    }

    #[test]
    fn a_full_pass_is_reproducible_from_its_seed() {
        // The whole normalised offer list, pinned to `SEED`: every draw the pass spends lands in
        // one of these fields, so a reordered or extra draw anywhere moves a number here.
        assert_eq!(
            normalised(&offers_of(Fixture::new()).offers),
            [
                (AMMO_BOX_TPL, 0, 2, false, ROUBLES, 5_500.0),
                (CHEAP_ROOT_TPL, 1, 1, false, DOLLARS, 1.0),
                (IN_RANGE_A_TPL, 2, 1, false, ROUBLES, 16_651.0),
                (TOO_CHEAP_TPL, 3, 4, false, ROUBLES, 106.0),
            ]
        );
    }

    #[test]
    fn an_expired_pass_replaces_one_for_one_and_never_runs_the_validity_check() {
        let mut fixture = Fixture::new();
        // BSG-sellable *and* custom-blacklisted: a full pass reaches the custom arm — the only one
        // that writes to `rejected` — and both rejects and records it. So an empty `rejected` here
        // can only mean the check never ran. A tpl the BSG arm rejects first would make the
        // assertion below vacuous.
        fixture
            .dynamic
            .blacklist
            .custom
            .insert(TOO_CHEAP_TPL.to_owned());
        // Three offers per assort for a fresh pass; an expired one is always a single replacement.
        fixture
            .dynamic
            .offer_item_count
            .insert("default".to_owned(), fixed(3));

        let result = generate_dynamic_offers(GenerateDynamicOffersRequest {
            expired_offers: Some(vec![vec![item("expired_root", TOO_CHEAP_TPL)]]),
            ..request(fixture)
        })
        .expect("the expired root is in the items view");

        // The validity check was never reached, so the custom-blacklist arm never fired. Asserted
        // first: it is the sharper of the two, and a validity check that did run would reject the
        // offer outright, so the golden below would fail for a second reason.
        assert!(result.rejected_can_sell_templates.is_empty());
        // Exactly one offer despite the `{3, 3}` range — and the seeded values below are what pins
        // the "no offer-count draw" half of that arm: an extra draw would move all of them.
        assert_eq!(
            normalised(&result.offers),
            [(TOO_CHEAP_TPL, 0, 2, false, ROUBLES, 110.0)]
        );
    }

    #[test]
    fn a_custom_blacklisted_template_is_recorded_and_makes_no_offer() {
        let mut fixture = Fixture::new();
        fixture
            .dynamic
            .blacklist
            .custom
            .insert(TOO_CHEAP_TPL.to_owned());

        let result = offers_of(fixture);

        assert_eq!(result.rejected_can_sell_templates, [TOO_CHEAP_TPL]);
        assert_eq!(
            result
                .offers
                .iter()
                .map(|offer| offer.items[0].template.as_str())
                .collect::<Vec<_>>(),
            [AMMO_BOX_TPL, CHEAP_ROOT_TPL, IN_RANGE_A_TPL]
        );
    }

    #[test]
    fn offers_are_numbered_from_the_requested_counter_start() {
        let result = generate_dynamic_offers(GenerateDynamicOffersRequest {
            offer_counter_start: 7,
            ..request(Fixture::new())
        })
        .expect("the fixture is complete");

        assert_eq!(
            result
                .offers
                .iter()
                .map(|offer| offer.internal_id)
                .collect::<Vec<_>>(),
            [7, 8, 9, 10]
        );
    }

    #[test]
    fn an_expired_offer_for_a_template_the_items_view_does_not_know_is_an_error() {
        let error = generate_dynamic_offers(GenerateDynamicOffersRequest {
            expired_offers: Some(vec![vec![item("ghost", "tpl_nothing_has_ever_heard_of")]]),
            ..request(Fixture::new())
        })
        .expect_err("the stack-count lookup is the C# throw");

        assert!(
            error.message.contains("not found in db"),
            "{}",
            error.message
        );
    }

    #[test]
    fn the_batch_reports_how_long_the_offer_pass_took() {
        let result = offers_of(Fixture::new());

        let last = result
            .diagnostics
            .last()
            .expect("the pass always reports its offer timing");
        let message = last.message.as_deref().unwrap_or_default();
        assert_eq!(last.level, "debug");
        assert!(
            message.starts_with("Took ") && message.ends_with("ms to CreateOffersFromAssort"),
            "{message}"
        );
    }

    // -----------------------------------------------------------------------
    // create_offers_from_assort
    // -----------------------------------------------------------------------

    /// An armor tpl the flea will sell: priced, and past the BSG-list arm.
    fn sellable_armor_fixture() -> Fixture {
        let mut fixture = Fixture::new();
        fixture.prices.insert(ARMOR_TPL.to_owned(), 20_000.0);
        fixture
            .items
            .get_mut(ARMOR_TPL)
            .expect("the armor tpl is in the view")
            .can_sell_on_ragfair = Some(true);
        fixture.presets.insert(
            "armor_preset".to_owned(),
            serde_json::from_value(json!({"id": "armor_preset", "encyclopedia": ARMOR_TPL,
                "items": []}))
            .expect("the preset parses"),
        );

        fixture
    }

    /// The armor tree as an assort entry, flagged as the preset `armor_preset`.
    fn armor_preset_assort() -> Vec<Item> {
        let mut assort = armor_with_plates();
        assort[0].upd = Some(Upd {
            extra: [("sptPresetId".to_owned(), json!("armor_preset"))]
                .into_iter()
                .collect(),
            ..Upd::default()
        });

        assort
    }

    fn offers_from_assort(
        fixture: &Fixture,
        assort: &mut Vec<Item>,
        is_expired_offer: bool,
    ) -> (Vec<RagfairOfferWire>, IndexSet<String>) {
        let mut offers = Vec::new();
        let mut rejected = IndexSet::new();
        let mut offer_counter = 0;

        seeded(|| {
            create_offers_from_assort(
                &mut fixture.ctx(),
                assort,
                is_expired_offer,
                &mut offers,
                &mut rejected,
                &mut offer_counter,
            )
        })
        .expect("the armor assort is sellable");

        (offers, rejected)
    }

    #[test]
    fn a_preset_assort_loses_its_banned_plates_once_for_the_whole_offer_loop() {
        let mut fixture = sellable_armor_fixture();
        fixture
            .dynamic
            .offer_item_count
            .insert("default".to_owned(), fixed(2));
        let mut assort = armor_preset_assort();

        let (offers, _) = offers_from_assort(&fixture, &mut assort, false);

        // The class 6 front plate is over the level cap; the class 6 back plate is in
        // `ignoreSlots` and the class 4 left plate is at the cap. Removed off the *shared* list,
        // before the offer loop clones it.
        assert_eq!(
            assort
                .iter()
                .map(|plate| plate.id.as_str())
                .collect::<Vec<_>>(),
            ["armor_root", "back", "left", "soft"]
        );
        // ...and then each offer's own clone loses the plates in `plateSlotIdToRemovePool`.
        assert_eq!(offers.len(), 2);
        for offer in &offers {
            assert_eq!(
                offer
                    .items
                    .iter()
                    .map(|plate| plate.slot_id.as_deref())
                    .collect::<Vec<_>>(),
                [None, Some("back_plate"), Some("soft_armor_front")]
            );
            // A preset lists as a single item, with no stack-count draw.
            assert_eq!(offer.quantity, 1);
        }
    }

    #[test]
    fn an_expired_preset_assort_keeps_its_banned_plates() {
        let fixture = sellable_armor_fixture();
        let mut assort = armor_preset_assort();

        let (offers, _) = offers_from_assort(&fixture, &mut assort, true);

        assert_eq!(assort.len(), 5);
        // Neither removal ran: banned plates are gated on `!isExpiredOffer`, and so is the
        // `RemoveArmorPlates` roll.
        assert_eq!(offers.len(), 1);
        assert_eq!(offers[0].items.len(), 5);
    }

    #[test]
    fn every_offer_of_one_assort_is_a_detached_clone_with_its_own_seller() {
        let mut fixture = sellable_armor_fixture();
        fixture
            .dynamic
            .offer_item_count
            .insert("default".to_owned(), fixed(2));
        // Keep the plates on, so there are children to compare.
        fixture.dynamic.armor.remove_removable_plate_chance = 0;
        fixture.dynamic.blacklist.enable_bsg_list = false;
        let mut assort = armor_preset_assort();

        let (offers, _) = offers_from_assort(&fixture, &mut assort, false);

        for offer in &offers {
            // The root's assort parentage is cleared; `ReparentItemAndChildren` keeps its id.
            assert_eq!(offer.items[0].id, "armor_root");
            assert_eq!(offer.items[0].parent_id, None);
            assert_eq!(offer.items[0].slot_id, None);
            // Children are re-ided under it.
            assert_eq!(offer.items[1].parent_id.as_deref(), Some("armor_root"));
            assert_ne!(offer.items[1].id, "front");
        }
        assert_ne!(offers[0].user.id, offers[1].user.id);
        assert_ne!(offers[0].items[1].id, offers[1].items[1].id);
        // The shared assort list is untouched by the per-offer re-id.
        assert_eq!(assort[1].id, "front");
    }

    // -----------------------------------------------------------------------
    // create_single_offer_for_item — the draw order
    // -----------------------------------------------------------------------

    #[test]
    fn a_barter_win_short_circuits_the_pack_chance_roll() {
        let mut fixture = Fixture::new();
        // Both rolls would fire; only the barter one is ever drawn.
        fixture.dynamic.barter.chance_percent = 100.0;
        fixture.dynamic.pack.chance_percent = 100.0;
        fixture
            .dynamic
            .pack
            .item_type_whitelist
            .insert(CHEAP_ROOT_TPL.to_owned());
        // A degenerate non-stackable range, so the stack-count arm spends no draw and the barter
        // roll is the call's first.
        fixture.dynamic.non_stackable_count = fixed(1);
        let items = vec![item("root", CHEAP_ROOT_TPL)];

        let after = stream_position_after(|| {
            let mut offers = Vec::new();
            let mut offer_counter = 0;
            create_single_offer_for_item(
                &mut fixture.ctx(),
                USER_ID,
                items.clone(),
                false,
                CHEAP_ROOT_TPL,
                false,
                OfferCreator::FakePlayer,
                &mut offers,
                &mut offer_counter,
            )
            .expect("the cheap root prices fine");
        });

        // The same draws, spelled out in order: the barter chance roll, the barter arm's own
        // draws, then the offer's. No pack roll sits between the first two.
        assert_eq!(
            after,
            stream_position_after(|| {
                let mut ctx = fixture.ctx();
                get_int(1, 99);
                let mut items = items.clone();
                randomise_offer_item_upd_properties(
                    &ctx,
                    &mut items,
                    CHEAP_ROOT_TPL,
                    OfferCreator::FakePlayer,
                )
                .expect("the cheap root has no condition entry");
                let barter_scheme =
                    create_barter_barter_scheme(&mut ctx, &items, &fixture.dynamic.barter)
                        .expect("the cheap root falls back to currency");
                create_offer(&mut ctx, &offer_details(items, barter_scheme), &mut 0)
                    .expect("the cheap root is in the view");
            })
        );
    }

    #[test]
    fn a_pack_offer_lists_a_random_pack_size_as_one_piece() {
        let mut fixture = Fixture::new();
        fixture.dynamic.pack.chance_percent = 100.0;
        fixture.dynamic.pack.item_count_min = 5;
        fixture.dynamic.pack.item_count_max = 5;
        // The whitelist is a *base class* list: an item is never its own base class.
        fixture
            .dynamic
            .pack
            .item_type_whitelist
            .insert(AMMO_BOX.to_owned());
        let mut offers = Vec::new();
        let mut offer_counter = 0;

        seeded(|| {
            create_single_offer_for_item(
                &mut fixture.ctx(),
                USER_ID,
                vec![item("root", AMMO_BOX_TPL)],
                false,
                AMMO_BOX_TPL,
                false,
                OfferCreator::FakePlayer,
                &mut offers,
                &mut offer_counter,
            )
        })
        .expect("the ammo box prices fine");

        assert_eq!(offers[0].quantity, 5);
        assert!(offers[0].sell_in_one_piece);
        // `CreateCurrencyBarterScheme(items, true, desiredStackSize)`: the listing is the whole
        // pack, so the per-item cost is a fifth of it.
        assert_eq!(
            offers[0].requirements_cost,
            round_half_even(offers[0].summary_cost / 5.0)
        );
    }

    #[test]
    fn a_barter_offer_resets_the_root_stack_when_make_single_stack_only_is_set() {
        let mut fixture = Fixture::new();
        fixture.dynamic.barter.chance_percent = 100.0;
        fixture.dynamic.barter.make_single_stack_only = true;
        let mut root = item("root", BARTER_ROOT_TPL);
        root.upd.as_mut().unwrap().stack_objects_count = Some(9.0);
        let mut offers = Vec::new();
        let mut offer_counter = 0;

        seeded(|| {
            create_single_offer_for_item(
                &mut fixture.ctx(),
                USER_ID,
                vec![root],
                false,
                BARTER_ROOT_TPL,
                false,
                OfferCreator::FakePlayer,
                &mut offers,
                &mut offer_counter,
            )
        })
        .expect("the barter root prices fine");

        assert_eq!(
            offers[0].items[0].upd.as_ref().unwrap().stack_objects_count,
            Some(1.0)
        );
        // A barter offer's requirement is an item, not a currency.
        assert!(
            [IN_RANGE_A_TPL, IN_RANGE_B_TPL]
                .contains(&offers[0].requirements[0].template_id.as_str())
        );
    }

    #[test]
    fn an_offer_for_an_empty_item_list_is_an_error() {
        let fixture = Fixture::new();
        let mut offers = Vec::new();
        let mut offer_counter = 0;

        let error = seeded(|| {
            create_single_offer_for_item(
                &mut fixture.ctx(),
                USER_ID,
                Vec::new(),
                false,
                CHEAP_ROOT_TPL,
                false,
                OfferCreator::FakePlayer,
                &mut offers,
                &mut offer_counter,
            )
        })
        .expect_err("the C# dereferences a null root item");

        assert_eq!(
            error.message,
            "Object reference not set to an instance of an object."
        );
    }
}
