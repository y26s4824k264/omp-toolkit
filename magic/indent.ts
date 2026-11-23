interface DetectedIndent {
	tabSize?: number;
	insertSpaces?: boolean;
}

/**
 * Sniff `insertSpaces` and the indent unit from `content`.
 *
 * Walks the buffer once: the first indented line decides spaces vs tabs; for
 * space indents, the most frequent indent increase between consecutive lines
 * gives the stride (so a 2/4/6 file reports `2`, a 4/8 file reports `4`).
 * Lines inside block comments are skipped: JSDoc continuations (` * …`) sit
 * one space past `/*`, not one indent level deeper. Taking the mode keeps
 * one-off alignment (template-literal bodies, hanging continuations) from
 * dragging the stride down. Returns `undefined` for any field the content does
 * not pin so a higher-precedence override (editorconfig) can win without being
 * overwritten by sniffing noise.
 */
export function detectIndentFromContent(content: string): DetectedIndent {
	if (content.length === 0) return {};

	let insertSpaces: boolean | undefined;
	let inBlockComment = false;
	let prevWidth = 0;
	// Indent increase (in spaces) → number of lines that stepped in by it.
	const steps = new Map<number, number>();

	for (const line of content.split("\n")) {
		const body = line.trim();
		// Blank/whitespace-only lines carry no indent signal.
		if (body.length === 0) continue;

		if (inBlockComment) {
			if (body.includes("*/")) inBlockComment = false;
			continue;
		}
		// The opener line itself is indented like code; only its continuations are skipped.
		if (body.startsWith("/*") && !body.includes("*/", 2)) inBlockComment = true;

		// Tab-indented line: the unit is one tab per level; tabSize is a
		// display concern, leave it to caller defaults / editorconfig.
		if (line[0] === "\t") {
			insertSpaces ??= false;
			continue;
		}

		// Space-indented (or unindented): count leading spaces, stopping at the
		// first tab to avoid mixing.
		let width = 0;
		while (line[width] === " ") width++;
		if (width > 0) insertSpaces ??= true;
		if (width > prevWidth) {
			const step = width - prevWidth;
			steps.set(step, (steps.get(step) ?? 0) + 1);
		}
		prevWidth = width;
	}

	const result: DetectedIndent = {};
	if (insertSpaces !== undefined) result.insertSpaces = insertSpaces;
	if (insertSpaces === true) {
		const unit = mostFrequentStep(steps);
		if (unit > 0) result.tabSize = unit;
	}
	return result;
}

/** Most frequent key in `steps`; ties go to the smaller step. `0` when empty. */
function mostFrequentStep(steps: Map<number, number>): number {
	let best = 0;
	let bestCount = 0;
	for (const [step, count] of steps) {
		if (count > bestCount || (count === bestCount && step < best)) {
			best = step;
			bestCount = count;
		}
	}
	return best;
}
