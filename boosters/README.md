# Booster definitions

Each `.toml` file in this folder defines one booster product: which sets it
applies to and what goes in each slot. The app reads this folder at startup,
so editing or adding a file needs no rebuild.

The folder is looked for in this order: the path in the
`MTGBOOSTERSIM_BOOSTERS` environment variable, `boosters/` in the working
directory, `boosters/` beside the executable. The app does not start without
it.

## Top-level fields

| Field | Meaning |
|---|---|
| `id` | Product family (`play`, `collector`, `draft`, ...). See "Per-set definitions". |
| `name` | Name shown in the app. |
| `description` | Shown when hovering over the choice. |
| `order` | Position among a set's choices; lower first. Default 100. |
| `exclusive` | If true and this matches a set, non-exclusive definitions are not offered for it. |
| `mythic_odds` | Chance a `rare_mythic` slot is a mythic. Default 0.125. |
| `wildcard_weights` | Common, uncommon, rare, mythic weights for `wildcard` slots. Default `[0.55, 0.30, 0.125, 0.025]`. |
| `theme` | `"color"` restricts every card in the pack to one random color. |
| `random_foils` | After the pack is built, this many random cards become foil. |

## `[[availability]]`

One or more rules; a set is covered when it satisfies every field of any one
rule.

| Field | Meaning |
|---|---|
| `first_set`, `last_set` | Informational only: the first and last sets sold in this product. Omit `last_set` while it is still current. |
| `from`, `before` | Release date range (`YYYY-MM-DD`); `from` is inclusive, `before` exclusive. |
| `set_types` | Scryfall set types, e.g. `["expansion", "core", "masters", "draft_innovation"]`. |
| `sets` | Explicit set codes, e.g. `["2xm", "2x2"]`. |
| `name_contains` | Text the set name must contain. |

## Per-set definitions

When several definitions with the same `id` cover a set, only one is offered:
a definition whose rule names the set in `sets` beats one that matches by date
or type. To give one set its own contents, copy the general file, keep the
`id`, and replace its availability with `sets = ["code"]`.
`masters_double.toml` is an example.

## `[[slots]]`

Slots are filled in order and shown in that order.

| Field | Meaning |
|---|---|
| `label` | Shown above the card. |
| `source` | Where the card comes from; see below. |
| `count` | Number of cards. Default 1. |
| `count_weights` | Random count instead, e.g. `{ 1 = 15, 2 = 45 }`: one card with weight 15, two with weight 45. |
| `complement_of_previous` | Count is this number minus the previous slot's count. |
| `foil` | The slot is foil. |
| `foil_odds` | Chance the slot is foil. |
| `treatment` | `regular` (default), `any`, `extended` or `showcase` (showcase or borderless). Falls back to any treatment, then to regular printings, when the set has none. |
| `treatment_odds` | Chance a regular card is swapped for a special printing of itself. |
| `placeholder` | Text shown when the source has no card (ad cards, missing art cards). Without it an empty slot is skipped. |
| `identical` | Every card in the slot is the same card. |
| `random_foil_eligible` | Set to false to keep `random_foils` off this slot. Default true. |

Sources: `common`, `uncommon`, `rare`, `mythic`, `rare_mythic`,
`common_uncommon`, `wildcard` (any rarity), `basic_land`, `special_land`
(full-art or special basic land), `token`, `art_card`, `special_guest`,
`list` (a random card from The List), `placeholder`.

A rarity that the set lacks falls back to the nearest one, and a land slot in
a set without basic lands is filled with a common. Cards are not repeated
within a pack while unused ones remain.

### `[[slots.replace]]`

Alternatives that can take the place of the slot's last card. One roll is made
per slot; each entry claims its `odds` share of it.

| Field | Meaning |
|---|---|
| `odds` | Chance this replacement happens. |
| `label` | Label for the replacement; defaults to the slot's label. |
| `source`, `treatment`, `foil`, `placeholder` | As for a slot. A foil replacement is skipped for sets not printed in foil. |
| `set_before` | Only applies to sets released before this date. |

Unknown fields and out-of-range odds are reported as errors at startup, naming
the file.
