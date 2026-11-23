# Magic editing source

The deterministic Magic editing engine and apply prompts are from [Oh My Pi v19.1.0](https://github.com/can1357/oh-my-pi/tree/a7135b8f31826c878734388b648f52bed8da436e), under the [MIT license](https://github.com/can1357/oh-my-pi/blob/a7135b8f31826c878734388b648f52bed8da436e/LICENSE). The distribution includes the upstream MIT copyright and license text.

| Local file | Upstream file |
| --- | --- |
| `magic-lines.ts` | `packages/coding-agent/src/edit/magic-lines.ts` |
| `magic-view.ts` | `packages/coding-agent/src/edit/magic-view.ts` |
| `magic-sections.ts` | `packages/tui/src/tools/magic-sections.ts` |
| `magic-apply.md`, `magic-apply-system.md` | `packages/coding-agent/src/edit/` |
| `indent.ts` | Pure indent detection and its helper from `packages/coding-agent/src/lsp/format-options.ts` |

The toolkit resolves the pure TypeScript modules directly on Node.js 24. The CLI adapts section merging, prompt rendering, snapshot normalization, and reply checks from `packages/coding-agent/src/edit/magic.ts`. The current AI generates line operations from the prepared prompt. The toolkit checks the result against the same project snapshot and produces a Git patch for review.

Local compiler changes preserve literal dollar signs in `sub` replacements, bound replacement growth, and restrict operation lookup to declared names.
