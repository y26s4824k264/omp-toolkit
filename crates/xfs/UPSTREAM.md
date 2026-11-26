# Upstream source

This crate was extracted from [Oh My Pi v19.0.0](https://github.com/can1357/oh-my-pi/tree/57771ae562b9345e0579bceb2d68d585e3e6927e/crates/xfs). The upstream code retains its [MIT license](LICENSE).

The Toolkit imports the following fixes and their tests from [Oh My Pi v19.1.0](https://github.com/can1357/oh-my-pi/tree/a7135b8f31826c878734388b648f52bed8da436e/crates/xfs):

- `src/exec.rs` propagates an input stream panic to the consumer in stream order. [Upstream commit](https://github.com/can1357/oh-my-pi/commit/5ec01cc78aa10b68ad77b5334f05d1d02c4eadac)
- `src/walk/arena.rs`, `src/walk/parallel.rs`, and `src/walk/tests.rs` keep queued paths alive when a parallel walk is dropped before its first worker runs. [Upstream commit](https://github.com/can1357/oh-my-pi/commit/e9ae95117a7f8fda9e3ad83a84c6e831106fe0ac)

The filesystem engine also includes support for linked worktrees' shared exclude rules from [v19.2.0](https://github.com/can1357/oh-my-pi/commit/58bf65d444ba882ee76cd0ca6cd6e1d05773013f) and caller-scoped walk and grep cancellation from [v19.2.1](https://github.com/can1357/oh-my-pi/commit/4f52dc0958f778049b0fe1f208e86d1e42b7b9ae). The standalone CLI drives the deadline and owns the cancellation scope.
