/**
 * Clyean agent identity helpers.
 *
 * Every Clyean agent runs the harness under its own profile with
 * `CLYEAN_AGENT=<agent-id>` in the environment, so model, MCP, and login
 * configuration is scoped to that agent by construction. These helpers let
 * user-facing text say which agent a command applies to.
 */

/** The Clyean agent identifier of this harness process, or undefined outside a Clyean sandbox. */
export function getClyeanAgent(env: NodeJS.ProcessEnv = process.env): string | undefined {
	const value = env.CLYEAN_AGENT?.trim();
	return value ? value : undefined;
}

/** Suffix such as " (agent: user-assistant)" for descriptions of agent-scoped commands, or "" outside Clyean. */
export function agentScopeSuffix(env: NodeJS.ProcessEnv = process.env): string {
	const agent = getClyeanAgent(env);
	return agent ? ` (agent: ${agent})` : "";
}
