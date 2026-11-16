# p6fixb divergences

New rows:

| `OffloadConditions.threshold` | A JS number: `NaN` and `±Infinity` are ignored, a fraction or a negative value is compared as is | A `u64` token count (`Option<u64>`), so those values can't be written | Token counts are integers, so a fractional threshold and its floor select the same blocks; a negative threshold differs from `0` only for a block of zero tokens (P-623) |
| `LocalFileStorage.write` dropped mid-write | Not applicable: a JS promise runs to completion | The temp write and rename run in one blocking task that finishes even when the caller's future is dropped, so no `__strands_tmp_` file is left | Rust futures can be dropped between the two steps |

No new row for the stash depth fix (correctness-6): stash entries now fall under the existing row `JSON nesting depth on read (saved data, model tool input)`.
