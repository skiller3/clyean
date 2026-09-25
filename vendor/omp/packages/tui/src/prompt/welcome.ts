import { TERMINAL } from "../terminal-capabilities";
import type { Component } from "../tui";
import { padding, replaceTabs, truncateToWidth, visibleWidth, wrapTextWithAnsi } from "../utils";
import { CLI_NAME, HARNESS_ATTRIBUTION } from "@oh-my-pi/pi-utils/dirs";
import { bgAnsi, fgAnsi } from "../theme/color";
import type { ColorMode } from "../theme/schema";
import { theme } from "../theme/theme";
import tipsText from "./tips.txt" with { type: "text" };

/** Tips embedded at build time, one per line; blanks dropped. */
const TIPS: readonly string[] = tipsText
	.split("\n")
	.map(line => line.trim())
	.filter(line => line.length > 0);

/**
 * Fixed number of session rows in the welcome box so its height stays stable
 * across recent-session updates.
 */
export const WELCOME_SESSION_SLOTS = 4;

/**
 * Fixed number of LSP-server rows, for the same reason. Overflow is sliced so
 * the box height is constant regardless of how many servers a project has.
 */
export const WELCOME_LSP_SLOTS = 4;

/** Trailing marker that flags a tip as a "what's new" callout. Stripped before
 *  wrapping (with any preceding whitespace) and replaced by {@link NEW_TAG_TEXT}
 *  painted as a shimmering rainbow. Non-global so `.test` stays stateless. */
const NEW_TIP_MARKER = /\s*\[NEW\]\s*$/;

/** Visible text rendered in place of {@link NEW_TIP_MARKER}. */
const NEW_TAG_TEXT = "NEW!";

/** Milliseconds for one full hue rotation of the rainbow "NEW!" tag. */
const NEW_GLOW_PERIOD_MS = 1500;

/** Selection weight for "[NEW]" tips; ordinary tips weigh 1, so a freshly added
 *  affordance surfaces this many times as often. */
const NEW_TIP_WEIGHT = 4;

/** Pick a tip from `tips`, biased toward "[NEW]" tips by {@link NEW_TIP_WEIGHT};
 *  `r` is a uniform sample in [0, 1). Returns "" when `tips` is empty.
 *  Exported for tests. */
export function pickWeightedTip(tips: readonly string[], r: number): string {
	if (tips.length === 0) return "";
	const weights = tips.map(tip => (NEW_TIP_MARKER.test(tip) ? NEW_TIP_WEIGHT : 1));
	const total = weights.reduce((sum, weight) => sum + weight, 0);
	let acc = r * total;
	for (let i = 0; i < tips.length; i++) {
		acc -= weights[i] ?? 1;
		if (acc < 0) return tips[i] ?? "";
	}
	return tips[tips.length - 1] ?? "";
}

type ColorEncoding = "ansi-16m" | "ansi-256";

/** Paint each glyph of {@link NEW_TAG_TEXT} on a moving HSL rainbow. `phase`
 *  rotates the hue offset cyclically; successive renders with increasing phase
 *  shimmer, while a fixed phase yields a still rainbow. */
function renderNewTag(phase: number, encoding: ColorEncoding): string {
	const bold = "\x1b[1m";
	const reset = "\x1b[0m";
	const wrapped = ((phase % 1) + 1) % 1;
	const chars = [...NEW_TAG_TEXT];
	let out = bold;
	let prev = "";
	for (let i = 0; i < chars.length; i++) {
		const hue = Math.round(((i / chars.length + wrapped) % 1) * 360);
		const color = Bun.color(`hsl(${hue}, 95%, 60%)`, encoding) ?? "";
		if (color !== prev) {
			out += color;
			prev = color;
		}
		out += chars[i];
	}
	return out + reset;
}
export function renderWelcomeTip(tip: string, boxWidth: number, phase = 0): string[] {
	const label = "Tip: ";
	const labelWidth = visibleWidth(label);
	const bodyBudget = boxWidth - 1 - labelWidth; // 1 = leading indent
	if (bodyBudget < 8) return [];

	const isNew = NEW_TIP_MARKER.test(tip);
	const body = isNew ? tip.replace(NEW_TIP_MARKER, "") : tip;

	const wrappedBody = wrapTextWithAnsi(replaceTabs(body), bodyBudget);
	if (wrappedBody.length === 0) return [];

	// Pull both colors from the active theme so the line stays readable on light
	// themes; the previous hardcoded `#b48cff` / `#9ccfff` pastels (plus a manual
	// `\x1b[2m` dim on the body) dropped to ~1.5:1 contrast on a white background.
	const continuationIndent = padding(labelWidth);
	const styledLabel = theme.fg("customMessageLabel", label);

	const lines = wrappedBody.map((line, index) => {
		const styledBody = theme.fg("muted", line);
		const content = index === 0 ? `${styledLabel}${styledBody}` : `${continuationIndent}${styledBody}`;
		return ` ${theme.italic(content)}`;
	});

	if (isNew) {
		// Append the rainbow tag to the final body line when it fits within the
		// box; otherwise drop it onto its own indented continuation line so the
		// styled glyphs never overflow or reflow the wrapped body.
		const encoding: ColorEncoding = TERMINAL.trueColor ? "ansi-16m" : "ansi-256";
		const tag = renderNewTag(phase, encoding);
		const tagWidth = 1 + visibleWidth(NEW_TAG_TEXT); // 1 = space separator
		const lastLine = lines[lines.length - 1];
		if (lastLine !== undefined && visibleWidth(lastLine) + tagWidth <= boxWidth) {
			lines[lines.length - 1] = `${lastLine} ${tag}`;
		} else {
			lines.push(` ${continuationIndent}${tag}`);
		}
	}

	return lines;
}

export interface RecentSession {
	name: string;
	timeAgo: string;
}

export interface LspServerInfo {
	name: string;
	status: "ready" | "error" | "connecting" | "available";
	fileTypes: string[];
}

/**
 * Premium welcome screen with the block-based Clyean soap-bar logo, a
 * two-column layout, and a full-width harness attribution band.
 */
export class WelcomeComponent implements Component {
	#animStart: number | null = null;
	#animTimer: Timer | null = null;
	#requestRender: (() => void) | null = null;
	// Tip randomness is latched once so the tip is stable across renders, but
	// the nerdfont-nag gate re-reads the live preset: the startup prepaint can
	// run under the default "unicode" preset before settings resolve the real
	// one, and a memoized nag would survive the switch to "nerd".
	#nagRoll: number | undefined;
	#tipRoll: number | undefined;
	// Render cache: the welcome box is the first transcript-area component, so
	// returning a stable array reference keeps the whole frame prefix stable.
	// Bypassed while the intro animation runs (every frame differs).
	#cachedWidth = -1;
	#cachedLines: string[] | undefined;

	constructor(
		private version: string,
		private modelName: string,
		private providerName: string,
		private recentSessions: RecentSession[] = [],
		private lspServers: LspServerInfo[] = [],
	) {}
	get tip(): string | undefined {
		this.#nagRoll ??= Math.random();
		this.#tipRoll ??= Math.random();
		if (theme.getSymbolPreset() === "unicode" && this.#nagRoll < 0.1) {
			return "Please use nerdfont 😭.";
		}
		return pickWeightedTip(TIPS, this.#tipRoll) || undefined;
	}

	invalidate(): void {
		this.#cachedWidth = -1;
		this.#cachedLines = undefined;
	}
	/** The intro keeps the welcome block mutable; settling lets it retire to history. */
	isTranscriptBlockFinalized(): boolean {
		return this.#animTimer == null;
	}

	/**
	 * Play a one-shot intro that sweeps the gradient through every phase
	 * before settling on the resting frame. Safe to call multiple times —
	 * subsequent calls reset and replay.
	 */
	playIntro(requestRender: () => void): void {
		this.#stopAnimation();
		this.#requestRender = requestRender;
		this.#animStart = performance.now();
		this.#requestRender();
		this.#animTimer = setInterval(() => {
			const elapsed = performance.now() - (this.#animStart ?? 0);
			if (elapsed >= INTRO_MS) {
				this.#stopAnimation();
			}
			this.#requestRender?.();
		}, INTRO_TICK_MS);
	}

	#stopAnimation(): void {
		if (this.#animTimer != null) {
			clearInterval(this.#animTimer);
			this.#animTimer = null;
		}
		this.#animStart = null;
		this.#requestRender = null;
		// The settled (resting) frame differs from the last intro frame.
		this.invalidate();
	}

	/**
	 * Redirect a running intro's render callback to a new target when a host
	 * remounts this component mid-animation.
	 * Returns true while the intro is still animating; false = no-op (settled).
	 */
	retargetIntro(requestRender: () => void): boolean {
		if (this.#animTimer == null) return false;
		this.#requestRender = requestRender;
		return true;
	}

	/** Stop the intro immediately and settle on the resting frame. Safe when idle. */
	stopIntro(): void {
		this.#stopAnimation();
	}

	/** Update the version embedded in the welcome border title. */
	setVersion(version: string): void {
		this.version = version;
		this.invalidate();
	}

	setModel(modelName: string, providerName: string): void {
		this.modelName = modelName;
		this.providerName = providerName;
		this.invalidate();
	}

	setRecentSessions(sessions: RecentSession[]): void {
		this.recentSessions = sessions;
		this.invalidate();
	}

	setLspServers(servers: LspServerInfo[]): void {
		this.lspServers = servers;
		this.invalidate();
	}

	render(termWidth: number): readonly string[] {
		const animating = this.#animStart != null;
		if (!animating && this.#cachedLines && this.#cachedWidth === termWidth) {
			return this.#cachedLines;
		}
		const lines = this.#renderLines(termWidth);
		if (animating) {
			this.#cachedLines = undefined;
			this.#cachedWidth = -1;
		} else {
			this.#cachedLines = lines;
			this.#cachedWidth = termWidth;
		}
		return lines;
	}

	#renderLines(termWidth: number): string[] {
		// Box dimensions - responsive with max width and small-terminal support
		const maxWidth = 100;
		const boxWidth = Math.min(maxWidth, Math.max(0, termWidth - 2));
		if (boxWidth < 4) {
			return [];
		}
		const dualContentWidth = boxWidth - 3; // 3 = │ + │ + │
		const preferredLeftCol = 26;
		const minLeftCol = BRAND_LOGO_WIDTH;
		const minRightCol = 20;
		// Dynamic model/provider labels are truncated inside the fixed column.
		// Letting them influence the responsive breakpoint changes the box height
		// when authoritative session data replaces the empty prepaint labels.
		const leftMinContentWidth = Math.max(minLeftCol, visibleWidth("Welcome back!"));
		const desiredLeftCol = Math.max(
			Math.min(preferredLeftCol, Math.max(minLeftCol, Math.floor(dualContentWidth * 0.35))),
			leftMinContentWidth,
		);
		const dualLeftCol =
			dualContentWidth >= minRightCol + 1
				? Math.min(desiredLeftCol, dualContentWidth - minRightCol)
				: Math.max(1, dualContentWidth - 1);
		const dualRightCol = Math.max(1, dualContentWidth - dualLeftCol);
		const showRightColumn = dualLeftCol >= leftMinContentWidth && dualRightCol >= minRightCol;
		const leftCol = showRightColumn ? dualLeftCol : boxWidth - 2;
		const rightCol = showRightColumn ? dualRightCol : 0;

		// Logo: pick a frame from the intro animation if active, else the resting frame.
		const logoColored = this.#currentLogoFrame();

		// Left column - centered content
		const leftLines = [
			"",
			this.#centerText(theme.bold("Welcome back!"), leftCol),
			"",
			...logoColored.map(l => this.#centerText(l, leftCol)),
			"",
			this.#centerText(theme.fg("muted", this.modelName), leftCol),
			this.#centerText(theme.fg("borderMuted", this.providerName), leftCol),
		];

		// Right column separator
		const separatorWidth = Math.max(0, rightCol - 2); // padding on each side
		const separator = ` ${theme.fg("dim", theme.boxRound.horizontal.repeat(separatorWidth))}`;

		// Recent sessions content
		const sessionLines: string[] = [];
		if (this.recentSessions.length === 0) {
			sessionLines.push(` ${theme.fg("dim", "No recent sessions")}`);
		} else {
			// Reserve width for the bullet prefix (" • ") and the trailing " (timeAgo)"
			// so the relative time is never the part that gets truncated. The name
			// absorbs whatever space is left.
			const bulletPrefix = ` ${theme.md.bullet} `;
			const prefixWidth = visibleWidth(bulletPrefix);
			for (const session of this.recentSessions.slice(0, WELCOME_SESSION_SLOTS)) {
				const timeSuffixRaw = ` (${session.timeAgo})`;
				const timeWidth = visibleWidth(timeSuffixRaw);
				const nameBudget = Math.max(1, rightCol - prefixWidth - timeWidth);
				const nameVis = visibleWidth(session.name);
				const name = nameVis > nameBudget ? truncateToWidth(session.name, nameBudget) : session.name;
				sessionLines.push(
					`${theme.fg("dim", bulletPrefix)}${theme.fg("muted", name)}${theme.fg("dim", timeSuffixRaw)}`,
				);
			}
		}
		// Pad to the fixed slot count so the box height doesn't depend on session count.
		while (sessionLines.length < WELCOME_SESSION_SLOTS) {
			sessionLines.push("");
		}

		// LSP servers content
		const lspLines: string[] = [];
		if (this.lspServers.length === 0) {
			lspLines.push(` ${theme.fg("dim", "No LSP servers")}`);
		} else {
			for (const server of this.lspServers.slice(0, WELCOME_LSP_SLOTS)) {
				const icon =
					server.status === "ready"
						? theme.styledSymbol("status.enabled", "success")
						: server.status === "available"
							? theme.styledSymbol("status.enabled", "dim")
							: server.status === "connecting"
								? theme.styledSymbol("status.pending", "muted")
								: theme.styledSymbol("status.error", "error");
				const exts = server.fileTypes.slice(0, 3).join(" ");
				lspLines.push(` ${icon} ${theme.fg("muted", server.name)} ${theme.fg("dim", exts)}`);
			}
		}
		// Pad to the fixed slot count so the box height doesn't depend on server count.
		while (lspLines.length < WELCOME_LSP_SLOTS) {
			lspLines.push("");
		}

		// Right column
		const rightLines = [
			` ${theme.bold(theme.fg("accent", "Tips"))}`,
			` ${theme.fg("dim", "#")}${theme.fg("muted", " for prompt actions")}`,
			` ${theme.fg("dim", "/")}${theme.fg("muted", " for commands")}`,
			` ${theme.fg("dim", "!")}${theme.fg("muted", " to run bash")}`,
			` ${theme.fg("dim", "$")}${theme.fg("muted", " to run python")}`,
			separator,
			` ${theme.bold(theme.fg("accent", "LSP Servers"))}`,
			...lspLines,
			separator,
			` ${theme.bold(theme.fg("accent", "Recent sessions"))}`,
			...sessionLines,
			"",
		];

		// Border characters (dim)
		const hChar = theme.boxRound.horizontal;
		const h = theme.fg("dim", hChar);
		const v = theme.fg("dim", theme.boxRound.vertical);
		const tl = theme.fg("dim", theme.boxRound.topLeft);
		const tr = theme.fg("dim", theme.boxRound.topRight);
		const bl = theme.fg("dim", theme.boxRound.bottomLeft);
		const br = theme.fg("dim", theme.boxRound.bottomRight);

		const lines: string[] = [];

		// Top border with embedded title
		const title = this.version ? ` ${CLI_NAME} v${this.version} ` : ` ${CLI_NAME} `;
		const titlePrefixRaw = hChar.repeat(3);
		const titleStyled = theme.fg("dim", titlePrefixRaw) + theme.fg("muted", title);
		const titleVisLen = visibleWidth(titlePrefixRaw) + visibleWidth(title);
		const titleSpace = boxWidth - 2;
		if (titleVisLen >= titleSpace) {
			lines.push(tl + truncateToWidth(titleStyled, titleSpace) + tr);
		} else {
			const afterTitle = titleSpace - titleVisLen;
			lines.push(tl + titleStyled + theme.fg("dim", hChar.repeat(afterTitle)) + tr);
		}

		// Content rows
		const maxRows = showRightColumn ? Math.max(leftLines.length, rightLines.length) : leftLines.length;
		for (let i = 0; i < maxRows; i++) {
			const left = this.#fitToWidth(leftLines[i] ?? "", leftCol);
			if (showRightColumn) {
				const right = this.#fitToWidth(rightLines[i] ?? "", rightCol);
				lines.push(v + left + v + right + v);
			} else {
				lines.push(v + left + v);
			}
		}
		// Attribution band: a full-width row set under the columns, wrapped to
		// the box so it survives narrow terminals.
		const innerWidth = boxWidth - 2;
		const teeRight = theme.fg("dim", theme.boxRound.teeRight);
		const teeLeft = theme.fg("dim", theme.boxRound.teeLeft);
		if (showRightColumn) {
			lines.push(
				teeRight + h.repeat(leftCol) + theme.fg("dim", theme.boxRound.teeUp) + h.repeat(rightCol) + teeLeft,
			);
		} else {
			lines.push(teeRight + h.repeat(leftCol) + teeLeft);
		}
		for (const line of renderAttributionLines(innerWidth)) {
			lines.push(v + line + v);
		}
		// Bottom border
		lines.push(bl + h.repeat(innerWidth) + br);

		// Randomly picked tip, rendered directly beneath the box.
		lines.push(...this.#renderTip(boxWidth));

		return lines;
	}

	/**
	 * Render the per-instance tip line: the `customMessageLabel`-themed `Tip:`
	 * label followed by a `muted` body, the whole line italicized. Returns `[]`
	 * when no tip is available or the box is too narrow to be useful.
	 */
	#renderTip(boxWidth: number): string[] {
		const tip = this.tip;
		if (!tip) return [];
		// A trailing "[NEW]" marker paints an animated rainbow "NEW!" tag. Derive
		// its hue phase from wall-clock time so it shimmers across the welcome
		// intro's re-render frames, then settles into a still rainbow once the box
		// caches its resting frame. Non-"[NEW]" tips ignore the phase entirely.
		const phase = NEW_TIP_MARKER.test(tip) ? performance.now() / NEW_GLOW_PERIOD_MS : 0;
		return renderWelcomeTip(tip, boxWidth, phase);
	}

	/** Center text within a given width */
	#centerText(text: string, width: number): string {
		const visLen = visibleWidth(text);
		if (visLen >= width) {
			return truncateToWidth(text, width);
		}
		const leftPad = Math.floor((width - visLen) / 2);
		const rightPad = width - visLen - leftPad;
		return padding(leftPad) + text + padding(rightPad);
	}

	/** Fit string to exact width with ANSI-aware truncation/padding */
	#fitToWidth(str: string, width: number): string {
		const visLen = visibleWidth(str);
		if (visLen > width) {
			const ellipsis = "…";
			const ellipsisWidth = visibleWidth(ellipsis);
			const maxWidth = Math.max(0, width - ellipsisWidth);
			let truncated = "";
			let currentWidth = 0;
			let inEscape = false;
			for (const char of str) {
				if (char === "\x1b") inEscape = true;
				if (inEscape) {
					truncated += char;
					if (char === "m") inEscape = false;
				} else if (currentWidth < maxWidth) {
					truncated += char;
					currentWidth++;
				}
			}
			return `${truncated}${ellipsis}`;
		}
		return str + padding(width - visLen);
	}

	/** Pick the logo frame for the current intro phase, or the resting frame. */
	#currentLogoFrame(): readonly string[] {
		if (this.#animStart == null) return REST_FRAME;
		const elapsed = performance.now() - this.#animStart;
		if (elapsed >= INTRO_MS) return REST_FRAME;
		return introLogoFrame(elapsed / INTRO_MS);
	}
}

/**
 * Wrap the harness attribution to the inner box width, one leading space per
 * line. Returns `[]` when the box is too narrow to show any text. Exported for
 * tests.
 */
export function renderAttributionLines(innerWidth: number): string[] {
	const bodyBudget = innerWidth - 2;
	if (bodyBudget < 8) return [];
	return wrapTextWithAnsi(HARNESS_ATTRIBUTION, bodyBudget).map(line => {
		const styled = ` ${theme.fg("muted", line)}`;
		return styled + padding(Math.max(0, innerWidth - visibleWidth(styled)));
	});
}

/**
 * Brand mark shared by the welcome and setup surfaces, drawn from
 * assets/clyean-logo.svg: a bar of soap with a glossy highlight band and three
 * bubbles rising from its corner. Each character is one square pixel: `.`
 * empty, `#` soap, `=` highlight band, and `p`, `c`, `v` the pink, cyan, and
 * violet bubbles. Each pair of pixel rows renders as one row of half-block
 * cells (see {@link paintLogo}).
 */
export const BRAND_LOGO: readonly string[] = [
	".................vv..",
	"................v..v.",
	"...pp.....ccc..v....v",
	"..p..p...c...c.v....v",
	"..p..p..c.....c.v..v.",
	"...pp...c.....c..vv..",
	"........c.....c......",
	".........c...c.......",
	"..........ccc........",
	".....................",
	"...###########.......",
	".##===========##.....",
	"#################....",
	"#################....",
	"#################....",
	"#################....",
	".###############.....",
	"...###########.......",
];

/** Width of {@link BRAND_LOGO} in terminal cells. */
export const BRAND_LOGO_WIDTH = Math.max(...BRAND_LOGO.map(row => row.length));

/** Height of {@link BRAND_LOGO} in terminal cells. */
export const BRAND_LOGO_HEIGHT = Math.ceil(BRAND_LOGO.length / 2);

/** Gradient positions that give each bubble its own color, as in the SVG. */
const BUBBLE_GRADIENT_T: Readonly<Record<string, number>> = { p: 0, v: 0.5, c: 1 };

/** White mixed into the highlight band at its ends and at its center. */
const HIGHLIGHT_EDGE = 0.25;
const HIGHLIGHT_PEAK = 0.65;

const RESET = "\x1b[0m";

type Rgb = readonly [number, number, number];

/** Multi-stop palette for the diagonal gradient. */
const GRADIENT_STOPS: ReadonlyArray<readonly [number, number, number]> = [
	[248, 79, 204], // oklch(0.7 0.24 340)
	[147, 98, 244], // oklch(0.62 0.21 295)
	[0, 219, 228], // oklch(0.81 0.14 200)
];

/** 256-color ramp fallback when truecolor isn't available. */
const GRADIENT_RAMP_256 = [206, 170, 134, 99, 69, 74, 44];

/** Half-width of the shine highlight band, expressed in gradient-t units. */
const SHINE_HALF_WIDTH = 0.18;

export interface ShineConfig {
	/** Overall opacity of the shine overlay, in [0, 1]. */
	strength: number;
	/** Center of the shine band along the diagonal, in [0, 1]. */
	pos: number;
}

/** Gradient color at a normalized position `t` (0..1) along the diagonal. */
function gradientRgb(t: number): Rgb {
	// 5-stop palette widens the visible color range and avoids the
	// deep-blue valley a naive HSL lerp falls into.
	const stops = GRADIENT_STOPS;
	const seg = t * (stops.length - 1);
	const i = Math.min(stops.length - 2, Math.floor(seg));
	const f = seg - i;
	const a = stops[i];
	const b = stops[i + 1];
	return [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f, a[2] + (b[2] - a[2]) * f];
}

/** Opacity of the sliding shine highlight at gradient position `t`, in [0, 1]. */
function shineIntensity(t: number, shine?: ShineConfig): number {
	if (!shine || shine.strength <= 0) return 0;
	return Math.max(0, 1 - Math.abs(t - shine.pos) / SHINE_HALF_WIDTH) * shine.strength;
}

function towardWhite([r, g, b]: Rgb, amount: number): Rgb {
	return [r + (255 - r) * amount, g + (255 - g) * amount, b + (255 - b) * amount];
}

/** Shift gradient position `t` by `phase`, wrapping at 1; a zero phase keeps `t` = 1 at the cyan end. */
function shiftGradient(t: number, phase: number): number {
	const normalizedPhase = ((phase % 1) + 1) % 1;
	return normalizedPhase === 0 ? t : (t + normalizedPhase) % 1;
}

/**
 * Resolve the gradient SGR foreground escape for a normalized position `t`
 * (0..1) along the diagonal, compositing the optional sliding shine highlight.
 * The setup splash paints its water with it (truecolor when available,
 * 256-color ramp otherwise).
 */
export function gradientEscape(t: number, shine?: ShineConfig): string {
	const intensity = shineIntensity(t, shine);
	if (TERMINAL.trueColor) {
		const [r, g, b] = towardWhite(gradientRgb(t), intensity);
		return `\x1b[38;2;${Math.round(r)};${Math.round(g)};${Math.round(b)}m`;
	}
	const ramp = GRADIENT_RAMP_256;
	let idx = Math.min(ramp.length - 1, Math.max(0, Math.floor(t * (ramp.length - 1) + 0.5)));
	// Promote to the brightest ramp slot when the shine band peaks here.
	if (intensity > 0.5) idx = ramp.length - 1;
	return `\x1b[38;5;${ramp[idx]}m`;
}

/**
 * Paint pixel art in the {@link BRAND_LOGO} format as rows of half-block cells,
 * `undefined` where a cell is empty. `soapT` maps a soap pixel (column `x`,
 * pixel row `y`) to its gradient position; the bubbles keep their own colors,
 * shifted by `phase`. The optional shine is composited over every pixel.
 */
export function paintLogo(
	pixels: readonly string[],
	soapT: (x: number, y: number) => number,
	phase = 0,
	shine?: ShineConfig,
): (string | undefined)[][] {
	const mode: ColorMode = TERMINAL.trueColor ? "truecolor" : "256color";
	const pixelColor = (x: number, y: number): Rgb | undefined => {
		const row = pixels[y] ?? "";
		const role = row[x] ?? ".";
		if (role === ".") return undefined;
		const bubbleT = BUBBLE_GRADIENT_T[role];
		const t = paletteT(bubbleT === undefined ? soapT(x, y) : shiftGradient(bubbleT, phase), mode);
		const color = towardWhite(gradientRgb(t), shineIntensity(t, shine));
		return role === "=" ? towardWhite(color, highlightAmount(row, x)) : color;
	};
	const width = Math.max(...pixels.map(row => row.length));
	const rows: (string | undefined)[][] = [];
	for (let y = 0; y < pixels.length; y += 2) {
		const cells: (string | undefined)[] = [];
		for (let x = 0; x < width; x++) {
			cells.push(halfBlockCell(pixelColor(x, y), pixelColor(x, y + 1), mode));
		}
		rows.push(cells);
	}
	return rows;
}

/** On 256-color terminals, snap `t` to the ramp's steps so the soap shows clean bands instead of palette noise. */
function paletteT(t: number, mode: ColorMode): number {
	if (mode === "truecolor") return t;
	const steps = GRADIENT_RAMP_256.length - 1;
	return Math.round(t * steps) / steps;
}

function halfBlockCell(top: Rgb | undefined, bottom: Rgb | undefined, mode: ColorMode): string | undefined {
	if (top && bottom) return `${fgAnsi(cssRgb(top), mode)}${bgAnsi(cssRgb(bottom), mode)}▀${RESET}`;
	if (top) return `${fgAnsi(cssRgb(top), mode)}▀${RESET}`;
	if (bottom) return `${fgAnsi(cssRgb(bottom), mode)}▄${RESET}`;
	return undefined;
}

function cssRgb([r, g, b]: Rgb): string {
	return `rgb(${Math.round(r)}, ${Math.round(g)}, ${Math.round(b)})`;
}

/** White mixed into a highlight pixel: strongest mid-band, fading toward both ends like the SVG's shine. */
function highlightAmount(row: string, x: number): number {
	let start = x;
	while (row[start - 1] === "=") start--;
	let end = x;
	while (row[end + 1] === "=") end++;
	const u = end === start ? 0.5 : (x - start) / (end - start);
	return HIGHLIGHT_EDGE + (HIGHLIGHT_PEAK - HIGHLIGHT_EDGE) * (1 - Math.abs(2 * u - 1));
}

/** Bounding box of the soap and highlight pixels, which the soap's gradient spans. */
function soapBounds(pixels: readonly string[]): { left: number; top: number; right: number; bottom: number } {
	const bounds = { left: Infinity, top: Infinity, right: -Infinity, bottom: -Infinity };
	pixels.forEach((row, y) => {
		for (let x = 0; x < row.length; x++) {
			if (row[x] !== "#" && row[x] !== "=") continue;
			bounds.left = Math.min(bounds.left, x);
			bounds.top = Math.min(bounds.top, y);
			bounds.right = Math.max(bounds.right, x);
			bounds.bottom = Math.max(bounds.bottom, y);
		}
	});
	return bounds;
}

/**
 * Render pixel art in the {@link BRAND_LOGO} format with the soap on the SVG's
 * diagonal gradient (top-left → bottom-right across the soap), plus an
 * optional sliding shine band. `phase` (0..1) shifts the gradient, wrapping at
 * 1. When `shine` is provided, a soft white highlight is composited on top,
 * centered at `shine.pos`.
 */
export function gradientLogo(pixels: readonly string[], phase = 0, shine?: ShineConfig): string[] {
	const soap = soapBounds(pixels);
	const xSpan = Math.max(1, soap.right - soap.left);
	const ySpan = Math.max(1, soap.bottom - soap.top);
	// SVG's (0,0) → (1,1) gradient projects both normalized axes equally:
	// top-right and bottom-left land on the purple midpoint.
	const soapT = (x: number, y: number) => shiftGradient(((x - soap.left) / xSpan + (y - soap.top) / ySpan) / 2, phase);
	return paintLogo(pixels, soapT, phase, shine).map(cells => cells.map(cell => cell ?? " ").join(""));
}

/** Total length of the intro animation. */
const INTRO_MS = 3000;
/** Render cadence during the intro (~30fps). */
const INTRO_TICK_MS = 33;
/** Number of full gradient rotations the sweep performs before settling. */
const INTRO_SWEEPS = 2.5;
/** Number of times the shine highlight crosses the diagonal across the intro. */
const INTRO_SHINE_TRAVERSALS = 3;

/**
 * Logo frame for a normalized intro progress in [0, 1).
 *
 * Ease-out cubic so the spin decelerates into the resting state. The gradient
 * sweeps backward through INTRO_SWEEPS full rotations (`eased == 1` → phase =
 * 0 = resting frame) while the shine traverses the diagonal at a steady pace,
 * decoupled from the gradient phase so the two layers parallax; its strength
 * fades with the same ease-out curve so the highlight is gone by the resting
 * frame.
 */
function introLogoFrame(progress: number): string[] {
	const eased = 1 - (1 - progress) ** 3;
	const phase = ((((1 - eased) * INTRO_SWEEPS) % 1) + 1) % 1;
	const shinePos = (((progress * INTRO_SHINE_TRAVERSALS) % 1) + 1) % 1;
	const shineStrength = (1 - eased) ** 1.5;
	return gradientLogo(BRAND_LOGO, phase, { strength: shineStrength, pos: shinePos });
}

/** Resting gradient frame, cached for re-renders outside of the intro. */
const REST_FRAME = gradientLogo(BRAND_LOGO, 0);
