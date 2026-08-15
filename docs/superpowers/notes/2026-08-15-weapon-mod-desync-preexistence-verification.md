# Task 4 follow-up — is the seed-42 `mod_mount_000` divergence pre-existing?

**Verdict: PRE-EXISTING (latent, masked).** The mod-pool order projection (`2adcc1d`, `64b1be9`,
`0ac2828`) introduced no new native/legacy divergence; it only removed a slot-enumeration-order
divergence that was masking a *weapon-mod draw-count* divergence that already existed at `b4489c4`.

Everything below was produced in throwaway worktrees (`/tmp/parity-preexist` @ `b4489c4`,
`/tmp/parity-head` @ `0ac2828`), both removed afterwards. The main checkout was never modified.

## Method

Both worktrees got the same three-line instrumentation:

1. `[Ignore]` removed from `TheSameSeedGeneratesEquivalentInventoryAtRandomisedLevels`.
2. `LootJsonAssert.AssertEqual` writes both normalized inventories to `$SPT_PARITY_DUMP` before the
   equality check (so passing cases dump too).
3. For the isolation experiment only: `_seeds` widened to 20 seeds, and
   `SPT_Data/configs/bot.json` → `equipment.pmc.randomisation[*].randomisedArmorSlots = []`.

## Finding 0 — a run-to-run nondeterminism that must be discounted first

Two identical runs at `b4489c4` do **not** produce identical dumps. The diffs are confined to the
loose-loot tail (`slotId` = `main` / `pocketN`, indices ≳72) and hit **legacy and native identically
within a run**, so they never cause a parity failure:

```
role=usec-level20-42-legacy   len 83/83  firstdiff 77   [77] pocket4 618a431d… vs 617fd91e…
role=usec-level20-1337-legacy len 84/84  firstdiff 78   [78] pocket3 544fb3f3… vs 544fb37f…
role=usec-level20-1337-native len 84/84  firstdiff 78   [78] pocket3 544fb3f3… vs 544fb37f… (same flip)
```

This is why the HEAD dumps in `/tmp/parity/` differ from a fresh `b4489c4` legacy run in the last
two backpack items — that is noise, not a code change. **Everything at index < ~72 (equipment,
weapon, weapon mods) is reproducible run to run.** All conclusions below use only that region.

## Finding 1 — at `b4489c4` the seed-42 bots are *entirely different bots*

`b4489c4`, `usec-level20`, seed 42, legacy (83 items) vs native (81 items), first diff at
`$.items[12]` as Task 1 recorded. But the divergence is not confined to plate order — the whole
loadout cascades:

```
idx  LEGACY                                      NATIVE
 11  5aa7cfc0… Headwear                          5aa7cfc0… Headwear            (same)
 12  657babc6… Helmet_ears        **             657baaf0… Helmet_top
 13  657bab6e… Helmet_back                       657bab6e… Helmet_back
 14  657baaf0… Helmet_top         **             657babc6… Helmet_ears
 15  5b44d222… ArmorVest          **             5b432b96… Earpiece
 16  656fb0bd… Front_plate        **             64be79c4… ArmorVest      (different vest!)
 …
 24  5644bd2b… FirstPrimaryWeapon **             (native's weapon is at idx 20)
     = weapon_izhmash_ak74n_545x39                5926bb21… = weapon_hk_mp5_9x19
```

Native generates an **MP5**; legacy generates the **AK-74N**. There is therefore no AK-74N on the
native side at `b4489c4` at all, so the seed-42 `mod_mount_000` extra item **cannot be observed
literally** at that commit — the RNG streams part company at the first randomised armor slot and
every later item is a different draw. The HEAD seed-42 item stream simply did not exist before the
projection.

That makes the literal question unanswerable by direct comparison. So the divergence *class* was
isolated instead.

## Finding 2 — isolation experiment: remove the masking seam, keep the commit

Setting `randomisedArmorSlots = []` for the level-15+ PMC buckets removes the armor-slot enumeration
seam entirely, letting the streams stay in sync into weapon generation at **both** commits. 20 seeds
× 2 roles = 40 cases, identical config and instrumentation on both sides:

| commit | result | failing seeds (both roles) | first diff |
|---|---|---|---|
| `b4489c4` (pre-projection) | Failed: **8**, Passed: 32 | 5, 13, 15, 16 | `[29]`, `[59]`, `[61]`, `[30]` |
| `0ac2828` (HEAD)           | Failed: **6**, Passed: 34 | 5, 15, 16     | `[29]`, `[61]`, `[30]` |

**HEAD's failure set is a strict subset of `b4489c4`'s.** The projection fixed seed 13 and introduced
nothing. Seeds 5, 15 and 16 fail at the *same index with byte-identical content* at both commits:

```
seed 5,  $.items[29]  (identical at b4489c4 and HEAD)
 28    606587bd… mod_charge            || 606587bd… mod_charge
 29 ** 5b1fb3e1… mod_magazine          || 5e21a3c6… mod_magazine     <-- native picks a different magazine
 30 ** 64b7af5a… cartridges            || 64b7af5a… cartridges

seed 16, $.items[30]  (identical at b4489c4 and HEAD)
 29    6895bf08… mod_charge            || 6895bf08… mod_charge
 30 ** 544a378f… mod_magazine          || 5d1340bd… mod_magazine     <-- same, different weapon

seed 15, $.items[61]  (identical at b4489c4 and HEAD; legacy 116 items, native 126)
 60    669fa48f… mod_barrel            || 669fa48f… mod_barrel
 61 ** 57ae0171… mod_scope             || 668fe5ec… mod_sight_front  <-- legacy spawns a mod native skips
```

Seed 15 is exactly the seed-42 symptom class: **one side takes a randomised weapon-mod slot the other
skips**, then everything after it cascades and the item counts diverge. It reproduces unchanged at
`b4489c4`. Seeds 5 and 16 are the mechanism Task 4 fingered — the randomised `mod_magazine` path
(`GetFilteredMagazinePoolByCapacity` → `GetFilteredModPool` → `ExhaustableArray`) landing on a
different tpl / consuming a different number of draws.

## Finding 3 — what the projection actually changed on the native side

Comparing `b4489c4`-hacked against HEAD-hacked dump for dump (40 cases × 2 sides), the only native
change outside the loose-loot noise band is **seed 13**, and it is a slot-*order* fix on a pistol
sub-mod pool (which is service-derived, so the projection does reach it):

```
usec seed 13 native, b4489c4(A) vs HEAD(B)
 59 ** 5a32aa0c… mod_sight_rear   || 56d5a661… mod_sight_front
 60 ** 56d5a661… mod_sight_front  || 56d5a77e… mod_sight_rear
 61 ** 56d5a2bb… mod_pistol_grip  || 57c9a891… mod_pistol_grip
```

Seed 13 fails at `b4489c4` and **passes** at HEAD. (Seed 15's cross-commit native diff at index 104
is `pocket4` loose loot — the Finding 0 noise, not the projection.)

This slightly corrects Task 4's reasoning: the projection is *not* wholly inert on the weapon path —
it reorders **sub-mod** slot enumeration (mods of mods, whose pools come from
`BotEquipmentModPoolService`) and there it strictly improved parity. Top-level weapon slots still
come from the bot JSON and are unaffected.

## Finding 4 — b4489c4-native vs HEAD-native, without the config hack

Not comparable in any useful sense: as Finding 1 shows, they are different bots (MP5 vs AK-74N,
different armor vest, 81 vs 81 items but sharing only indices 0–11). The answer to "did the
projection change anything besides plate order" is therefore taken from the hacked sweep
(Finding 3): **no — the only non-noise native change across 40 cases is the seed-13 sub-mod slot
order fix.**

## Conclusion

- The seed-42 AK-74N `mod_mount_000` item stream is **new** — that exact bot could not exist before
  the plate fix. Its *defect* is not new.
- The defect class — native and legacy disagreeing on a randomised weapon-mod slot's spawn decision
  or pool selection — is demonstrably **pre-existing at `b4489c4`**, reproduced with byte-identical
  symptoms at both commits on seeds 5, 15 and 16 once the armor seam is removed.
- The projection commits introduced **zero** new divergences across 40 isolated cases and fixed one
  (seed 13). HEAD's failure set ⊂ `b4489c4`'s failure set.
- Task 4's blocked-status conclusion stands: the remaining work is root-causing a draw-count desync
  in the randomised weapon-mod path (`mod_magazine` selection is the prime suspect — it is the
  visible symptom on two of the three surviving isolated failures). That is not a regression from
  Tasks 2/3.

## Reproduction notes

- Worktrees `/tmp/parity-preexist` (`b4489c4`) and `/tmp/parity-head` (`0ac2828`), both removed.
- `scripts/decompress-assets.sh` needed in each worktree; `cargo` on `PATH`; runs take ~7 s after the
  first build.
- Dump dirs left on disk: `/tmp/parity-b4489c4`, `/tmp/parity-b4489c4-run2` (stock config, 4 cases),
  `/tmp/hack-b4489c4`, `/tmp/hack-head` (armor randomisation disabled, 40 cases).
