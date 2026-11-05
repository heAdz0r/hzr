# HZR 0.12.2

`hzr doctor` passes its pricing check again, and `hzr stats` can put a price on
saved tokens. The built-in price catalog was re-verified on 2026-11-05.

## What broke

HZR prices saved tokens from a built-in catalog of public API prices. Each entry
is trusted for 30 days after it was read. The catalog had been read on
2026-09-26, so from 2026-10-26 every price was reported stale: `hzr doctor`
failed `billing_pricing`, and the potential-value line of the session report
showed "pricing unavailable".

## What changed

- Every entry was read again from its provider's official page. All prices that
  remain in the catalog are unchanged.
- New entries: Claude Sonnet 5.5, Claude Haiku 5.5 (two prompt-size bands: up to
  100K and over 100K tokens), GPT-6.1 Sol and GPT-6.1 Terra, Mistral Large 4, and
  Gemini 4 Argon (its promotional rate through 2026-11-30 and the published rate
  from 2026-12-01).
- GPT-5.6 Sol's promotional rate is now valid through 2026-11-30.
- Mistral Large 3 and Mistral Medium 3.5 were removed: both are deprecated and no
  longer appear on Mistral's price list. A configuration that names them now
  reports pricing as unavailable rather than using an unverified price.

## Upgrade

Run `hzr update` or use the installer. If you price Claude Haiku 5.5, set the
billing method to `standard_prompt_lte_100k` or `standard_prompt_gt_100k`. The
catalog is trusted until 2026-12-05.
