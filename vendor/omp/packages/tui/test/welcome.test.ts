import { afterEach, beforeAll, describe, expect, it, vi } from "bun:test";
import {
	BRAND_LOGO,
	BRAND_LOGO_HEIGHT,
	BRAND_LOGO_WIDTH,
	pickWeightedTip,
	renderLogo,
	WelcomeComponent,
} from "@oh-my-pi/pi-tui/prompt/welcome";
import { initTheme, theme } from "@oh-my-pi/pi-tui/theme";
import { visibleWidth } from "@oh-my-pi/pi-tui/utils";

describe("WelcomeComponent", () => {
	beforeAll(async () => {
		await initTheme(false);
	});

	afterEach(() => {
		vi.restoreAllMocks();
	});

	it("selects standard tip when preset is not unicode", () => {
		vi.spyOn(theme, "getSymbolPreset").mockReturnValue("nerd");

		const welcome = new WelcomeComponent("1.0.0", "model", "provider");
		expect(welcome.tip).not.toBe("Please use nerdfont 😭.");
		expect(welcome.tip).toBeDefined();
	});

	it("selects nerdfont tip with 10% probability under unicode preset", () => {
		vi.spyOn(theme, "getSymbolPreset").mockReturnValue("unicode");

		// 9% chance => selects special tip
		vi.spyOn(Math, "random").mockReturnValue(0.09);
		const welcomeSpecial = new WelcomeComponent("1.0.0", "model", "provider");
		expect(welcomeSpecial.tip).toBe("Please use nerdfont 😭.");

		// 10% chance => selects regular tip
		vi.spyOn(Math, "random").mockReturnValue(0.1);
		const welcomeRegular = new WelcomeComponent("1.0.0", "model", "provider");
		expect(welcomeRegular.tip).not.toBe("Please use nerdfont 😭.");
		expect(welcomeRegular.tip).toBeDefined();
	});

	it("weights [NEW] tips above ordinary tips in selection", () => {
		// Data-independent: tips.txt may legitimately carry zero "[NEW]" tips, so
		// exercise the weighting contract on a synthetic list.
		const tips = ["plain one", "shiny thing [NEW]", "plain two"] as const;

		const counts = new Map<string, number>();
		const samples = 10_000;
		for (let i = 0; i < samples; i++) {
			const tip = pickWeightedTip(tips, (i + 0.5) / samples); // sweep the selection domain uniformly
			counts.set(tip, (counts.get(tip) ?? 0) + 1);
		}

		let newMax = 0;
		let ordinaryMax = 0;
		for (const [tip, count] of counts) {
			if (/\[NEW\]\s*$/.test(tip)) newMax = Math.max(newMax, count);
			else ordinaryMax = Math.max(ordinaryMax, count);
		}

		// A "[NEW]" tip carries a >1 weight, so it covers strictly more of the
		// uniform selection domain than any single ordinary tip.
		expect(newMax).toBeGreaterThan(0);
		expect(newMax).toBeGreaterThan(ordinaryMax);
		expect(pickWeightedTip([], 0.5)).toBe("");
	});

	it("truncates a long model name inside the fixed left column and keeps the right column", () => {
		// Dynamic model labels must not influence the responsive breakpoint: a
		// long name is truncated with an ellipsis instead of collapsing the right
		// column or changing the box height when authoritative session data
		// replaces the prepaint labels.
		const modelName = "DeepSeek V4 Flash (2x usage)";
		const output = new WelcomeComponent("17.3.4", modelName, "opencode-go").render(55).join("\n");
		const plain = output.replace(/\x1b\[[0-9;]*m/g, "");

		expect(plain).not.toContain(modelName);
		expect(plain).toMatch(/DeepSeek V4 [^│]*…/);
		expect(plain).toContain("Recent sessions");
	});

	it("titles the box with the Clyean version, or with the bare name outside Clyean", () => {
		const versioned = Bun.stripANSI(new WelcomeComponent("0.4.2", "", "").render(100)[0] ?? "");
		expect(versioned).toContain(" Clyean v0.4.2 ");

		const bare = Bun.stripANSI(new WelcomeComponent("", "", "").render(100)[0] ?? "");
		expect(bare).toMatch(/─ Clyean ─/);
		expect(bare).not.toContain("Clyean v");
	});

	it("leaves two blank rows between the greeting and the logo", () => {
		const rows = new WelcomeComponent("1.0.0", "", "").render(100).map(row => Bun.stripANSI(row));
		const greeting = rows.findIndex(row => row.includes("Welcome back!"));
		const logoTop = rows.findIndex(row => row.includes(Bun.stripANSI(renderLogo(BRAND_LOGO)[0] ?? "").trim()));
		expect(logoTop - greeting).toBe(3);
	});

	it("keeps the whole logo in the left column on narrow terminals", () => {
		const output = Bun.stripANSI(new WelcomeComponent("1.0.0", "", "").render(48).join("\n"));
		for (const row of renderLogo(BRAND_LOGO)) {
			expect(output).toContain(Bun.stripANSI(row).trim());
		}
	});
});

describe("renderLogo", () => {
	it("pairs pixel rows into half-block cells", () => {
		expect(renderLogo(["#.b.", "##.."]).map(row => Bun.stripANSI(row))).toEqual(["▀▄▀ "]);
	});

	it("renders the brand logo at its declared cell size", () => {
		const rows = renderLogo(BRAND_LOGO);
		expect(rows).toHaveLength(BRAND_LOGO_HEIGHT);
		for (const row of rows) {
			expect(visibleWidth(row)).toBe(BRAND_LOGO_WIDTH);
		}
	});
});
