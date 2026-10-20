# Public test replays

These three replays are the public fixtures vrfkit's CI uses. They come from
[michel-giehl/ValorantReplayParser](https://github.com/michel-giehl/ValorantReplayParser)
at commit `b51d67423b7b4952d59051cf91e55efa1c42da05`
(`tests/Test.Integration/Replays/`), MIT License; see
`LICENSE.ValorantReplayParser`.

| File | Build | SHA-256 |
|---|---|---|
| `9f8b32c5-c243-41ec-bbbb-832582edf652.12_10.vrf` | 12.10 | `a4ce1b72f9bdf99492162013c1c909e6994a0d22bef1899e687fde71fbc86606` |
| `5c673443-5bdc-4576-b416-aab3f62471a5.12_11.vrf` | 12.11 | `7a7a5492ddf286bb04413da96f0d3b216f91150e8174a3a4397493529e17ebdd` |
| `12974d2b-848f-490d-80ba-5f03a033c2d5.13_00.vrf` | 13.00 | `fd49091dd43171bb060eb6bbae50ed6677aa1077344572c5bf65f0c6fe2b4c1a` |

The hashes match the ones pinned in vrfkit's `.github/workflows/ci.yml`.

They are short solo custom-game clips: one player, no kills, 23 to 70 s. They
exercise scanning, storage and rendering on three older builds and on
degenerate data (no kills, no teams, no plants); the facts themselves are
checked against the oracle replay in `crates/core/tests/oracle.rs`.
