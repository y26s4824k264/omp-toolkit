# Upstream source

This crate was extracted from [Oh My Pi v19.0.0](https://github.com/can1357/oh-my-pi/tree/57771ae562b9345e0579bceb2d68d585e3e6927e/crates/edit). The upstream code retains its [MIT license](../../LICENSE).

The Toolkit imports `src/path_policy.rs` and `tests/replace_parity.rs` from [Oh My Pi v19.1.0](https://github.com/can1357/oh-my-pi/tree/a7135b8f31826c878734388b648f52bed8da436e/crates/edit). [The upstream fix](https://github.com/can1357/oh-my-pi/commit/986981bf53b7e528d542ca9c4f1c2ade5953dc49) keeps the same snapshot key before and after a path is created under a missing parent. Hashline previews use this key to match paths to the same file.

The Toolkit also imports the Sloppy source under `src/modes/sloppy/`, its grammar, prompt, fixtures, and tests from that tag. These changes add anchored block actions, reject incomplete or ambiguous edit ranges, and protect JSDoc openers during replacement. [Block actions and range validation](https://github.com/can1357/oh-my-pi/commit/aa70b716c5ef354bf333c611d2b88a3f9f1fe710) · [JSDoc protection](https://github.com/can1357/oh-my-pi/commit/b334112c7b8ae54a28e1a99cdec214b8d93e2770)
