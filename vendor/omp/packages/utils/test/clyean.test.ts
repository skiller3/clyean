import { describe, expect, it } from "bun:test";
import { getClyeanVersion } from "@oh-my-pi/pi-utils/clyean";

describe("getClyeanVersion", () => {
	it("reads the version the clyean host program passes in CLYEAN_VERSION", () => {
		expect(getClyeanVersion({ CLYEAN_VERSION: " 0.4.2 " })).toBe("0.4.2");
	});

	it("is undefined outside a Clyean sandbox", () => {
		expect(getClyeanVersion({})).toBeUndefined();
		expect(getClyeanVersion({ CLYEAN_VERSION: "  " })).toBeUndefined();
	});
});
