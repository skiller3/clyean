import { describe, expect, it } from "bun:test";
import { BUILTIN_SLASH_COMMAND_RESERVED_NAMES } from "@oh-my-pi/pi-coding-agent/slash-commands/builtin-registry";
import tipsText from "../../tui/src/prompt/tips.txt" with { type: "text" };

const tips = tipsText
	.split("\n")
	.map(line => line.trim())
	.filter(line => line.length > 0);

describe("welcome tips", () => {
	it("name only slash commands that Clyean registers", () => {
		for (const tip of tips) {
			for (const [, name] of tip.matchAll(/(?:^|[\s(`])\/([a-z][\w-]*)/g)) {
				expect(BUILTIN_SLASH_COMMAND_RESERVED_NAMES.has(name ?? ""), tip).toBe(true);
			}
		}
	});

	it("name the clyean command instead of upstream's omp", () => {
		for (const tip of tips) {
			expect(tip).not.toMatch(/\bomp\b/);
		}
	});
});
