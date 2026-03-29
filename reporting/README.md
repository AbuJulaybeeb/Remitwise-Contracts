# Reporting Contract

Financial reporting and insights contract for the RemitWise platform.

## Overview

Generates on-chain financial health reports by aggregating data from the
remittance\_split, savings\_goals, bill\_payments, and insurance contracts.

## Dependency contract address integrity

Reporting stores five downstream contract IDs (`remittance_split`, `savings_goals`,
`bill_payments`, `insurance`, `family_wallet`) set via `configure_addresses`.

**Validation (on every `configure_addresses` call)**:

- **No self-reference** — None of the five addresses may equal the reporting
  contract’s own address. Pointing a role at this contract would create ambiguous
  cross-contract calls and break the intended “one deployment per role” model.
- **Pairwise uniqueness** — All five values must differ. Two roles must not share
  the same contract ID, or aggregation would silently read the wrong deployment
  twice (audit and correctness risk).

**`verify_dependency_address_set`** exposes the same checks without writing
storage and without requiring authorization. Use it from admin UIs or scripts to
pre-validate a bundle before submitting a configuration transaction.

**Error**: `InvalidDependencyAddressConfiguration` (`6`) when the proposed set
is rejected.

**Security notes**:

- Validation is **O(1)** (fixed five slots, constant comparisons).
- This does **not** prove each address is the *correct* Remitwise deployment for
  its role (that requires off-chain governance / deployment manifests). It only
  enforces **structural** integrity: distinct callees and no reporting
  self-loop.
- Soroban/Stellar contract IDs are not an EVM-style “zero address”; “malformed”
  in this layer means duplicate or self-reference as above.

## Trend Analysis

### `get_trend_analysis`

Compares two scalar amounts and returns a `TrendData` struct:

```
TrendData {
    current_amount:    i128,
    previous_amount:   i128,
    change_amount:     i128,   // current - previous
    change_percentage: i32,    // signed %; 100 when previous == 0 and current > 0
}
```

**Determinism guarantee**: output depends only on `current_amount` and
`previous_amount`; ledger timestamp, user address, and call order have no
effect.

### `get_trend_analysis_multi`

Accepts a `Vec<(u64, i128)>` of `(period_key, amount)` pairs and returns
`Vec<TrendData>` with one entry per adjacent pair (`len - 1` entries).
Returns an empty Vec when fewer than two points are supplied.

**Determinism guarantee**: identical `history` input always produces identical
output regardless of call order, ledger state, or caller identity.

