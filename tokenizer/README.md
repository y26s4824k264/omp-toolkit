# Offline tokenizer sources

The OpenAI encodings use `tiktoken-rs` 0.12.1. The crate archive omits the upstream license, so the toolkit retains the [license from its packaged source revision](https://github.com/zurawiki/tiktoken-rs/blob/72a5a800651a1cea1ad609292446ba02c80e3bcd/LICENSE) and the [OpenAI tiktoken license](https://github.com/openai/tiktoken/blob/0.14.0/LICENSE) for its vendored code. Both are MIT.

The Claude code, vocabulary blobs, and regression fixtures in `claude/` come from [Oh My Pi v19.0.0, commit `57771ae562b9345e0579bceb2d68d585e3e6927e`](https://github.com/can1357/oh-my-pi/tree/57771ae562b9345e0579bceb2d68d585e3e6927e/crates/natives). The MIT notice for the compact vocabulary credits [sanderland/ctok, revision `df3b59b5e645289a5eadc8e24036b99d39c333c4`](https://github.com/sanderland/ctok/tree/df3b59b5e645289a5eadc8e24036b99d39c333c4) and the Oh My Pi contributors. See [engine license](claude/LICENSE) and [vocabulary license](claude/data/LICENSE.ctok).

The two reconstruction tables remain byte-for-byte identical to the upstream compressed assets. `ctok_v3.bin.zst` has SHA-256 `8791577f60f6c5cfd8b9c85c7cd560c6040e2dd25b9fbe5f6feac7e35e9748b1`. `ctok_v4_7.bin.zst` has SHA-256 `6083d7b803771bc55c537cc3a928d36fcc7104b3c802909e2a73d2bf15f7a458`.

All six encodings measure plain text. The Claude results are offline reconstructions of content counts and exclude chat framing. The implementation embeds its vocabulary data and calls neither an agent host nor an external service.
