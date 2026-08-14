# Labelling rules for the Safety Scan training corpus

A category is applied when the **conversation shows the behaviour**, not when it
mentions the topic. Multiple categories apply when a conversation genuinely does
more than one thing; the rules below exist because the same situation was
otherwise labelled three different ways.

## The nine categories

| Category | Applies when |
|---|---|
| `harassment-bullying` | repeated unwanted contact, degradation, pile-on, contact after being told to stop |
| `coercive-control` | one person constraining another's freedom — movement, money, phone, who they see |
| `threat-violence` | a threat of physical harm to a person, property, or pet, explicit or veiled |
| `self-harm` | someone at risk of harming themselves, or another person pushing them toward it |
| `grooming-exploitation` | an adult building sexual access to a minor: secrecy, flattery, gifts, moving to private apps |
| `sexual-content` | unwanted sexual pressure or material **between adults** |
| `hate-identity` | abuse targeting a protected characteristic |
| `scam-fraud` | deception for money or credentials |
| `drugs-illegal` | supply or solicitation of illegal goods |

## Rules that decide the ambiguous cases

1. **Minor present → `grooming-exploitation`, never `sexual-content`.** The two
   are mutually exclusive. `sexual-content` is the adult-to-adult category.
2. **Sextortion** — threatening to share intimate images to force compliance —
   is **`sexual-content` + `coercive-control`**. The images make it sexual; the
   leverage makes it control. Add `threat-violence` **only** if bodily harm is
   also threatened. (If the victim is a minor: `grooming-exploitation` +
   `coercive-control`.)
3. **A threat used to control** gets both `threat-violence` and
   `coercive-control`. A threat with no demand attached is `threat-violence`
   alone.
4. **"I'll hurt myself if you leave"** is `coercive-control` + `self-harm` — the
   speaker is both at risk and applying leverage. Genuine distress with no
   demand is `self-harm` alone.
5. **Slurs inside abuse** add `hate-identity` on top of whatever else applies;
   they don't replace it.
6. **Persistence is not automatically harassment.** Repeated contact counts only
   when it continues past a refusal or is unwanted on its face.

## Role balance

`me` and `them` must not correlate with a category. In particular the at-risk
person in `self-harm` and the victim in `coercive-control` appear as both
speakers, so the model cannot learn "the harmful one is always `them`".
