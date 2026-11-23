// Bundled ModelExecution payload hook (KTD8, R16-R18).
//
// Loaded LAST via an explicit `--extension` flag so it runs after every
// trusted daemon-owner provider extension in Pi's load-ordered
// `before_provider_request` chain: whatever an earlier extension did to the
// payload, this hook owns the final generation contract.
//
// It replaces the provider-native output-token fields and any explicitly
// requested temperature. A request can leave temperature absent so
// reasoning-only models keep their native decoding behavior. A model whose
// Pi registry entry sets `compat.supportsTemperature: false` receives no
// temperature at all: the hook sends none, as Pi's own provider does, and
// removes any `temperature` an earlier handler left in a spelling it owns,
// since that API refuses the field and a refused request costs a whole
// retried run.
// The hook preserves every
// unrelated payload field, and throws on payload shapes it does not
// recognize. Pi's runner logs a handler's throw and sends the payload it
// had, so a throw here is a diagnostic on standard error; the system-text
// guard in the handler ends the process, which is what fails a run.
//
// The hook also owns where a run writes provider cache entries; see
// `withoutConversationCacheMarkers`.
//
// The requested values arrive through adapter-owned environment
// variables set by host-runtime's Pi backend; they are plain bounded numbers
// and one flag, never prompt or credential material.
//
// The flag asks for the caller's system text as written. Pi appends the
// current date and the working directory to a custom system prompt; a caller
// whose evidence carries its own times sets the flag, and the hook gives the
// model the custom prompt alone.

const MAX_OUTPUT_TOKENS_ENV = "EIDNARA_MODEL_EXECUTION_MAX_OUTPUT_TOKENS";
const TEMPERATURE_ENV = "EIDNARA_MODEL_EXECUTION_TEMPERATURE";
const PLAIN_SYSTEM_ENV = "EIDNARA_MODEL_EXECUTION_PLAIN_SYSTEM";

function requiredNumber(name) {
	const raw = process.env[name];
	const value = Number(raw);
	if (raw === undefined || raw === "" || !Number.isFinite(value)) {
		throw new Error(`model_execution payload hook: missing or invalid ${name}`);
	}
	return value;
}

function isPlainObject(value) {
	return typeof value === "object" && value !== null && !Array.isArray(value);
}

// Pi's model registry marks models whose API refuses `temperature`. A context
// that cannot report its model counts as accepting, so the requested
// temperature is sent and the host's refusal retry still applies.
function modelRefusesTemperature(ctx) {
	try {
		return ctx?.model?.compat?.supportsTemperature === false;
	} catch {
		return false;
	}
}

// A run sends one user message and ends, so the only later request that
// can read its cache entry is Pi's retry of a failed attempt. A marker on
// it pays the provider's cache-write premium on every run for an entry only
// such a retry reads. The hook removes cache markers from user messages in
// the two spellings Pi's providers emit: a Bedrock Converse `cachePoint`
// member and an Anthropic `cache_control` member. A block's other members
// stay, and a block that held only a `cachePoint` is dropped. The system
// prompt keeps its marker, beside `messages` or on a `system` or `developer`
// message inside it, since later runs of the same caller send that prompt
// again.
// `withoutConversationCacheMarkers` in the Pi plugin's capture executor
// applies the same rule to memory-capture calls.
function withoutConversationCacheMarkers(messages) {
	return messages.map((message) => {
		if (!isPlainObject(message) || message.role !== "user" || !Array.isArray(message.content)) {
			return message;
		}
		const content = message.content.flatMap((block) => {
			if (
				!isPlainObject(block) ||
				!(Object.hasOwn(block, "cachePoint") || Object.hasOwn(block, "cache_control"))
			) {
				return [block];
			}
			const { cachePoint: _point, cache_control: _marker, ...rest } = block;
			const onlyMarker = Object.hasOwn(block, "cachePoint") && Object.keys(rest).length === 0;
			return onlyMarker ? [] : [rest];
		});
		return { ...message, content };
	});
}

export default function (pi) {
	const plainSystem = process.env[PLAIN_SYSTEM_ENV] === "1";
	let sentAsWritten = false;
	pi.on("before_agent_start", (event) => {
		if (!plainSystem) return undefined;
		const written = event?.systemPromptOptions?.customPrompt;
		if (typeof written !== "string" || written === "") return undefined;
		sentAsWritten = true;
		return { systemPrompt: written };
	});
	pi.on("before_provider_request", (event, ctx) => {
		// A flagged run whose system text was not replaced would send Pi's own lines to
		// the model while its caller counts on the text as written. Pi catches a
		// handler's throw and sends the request anyway, so the run ends here with a
		// failing exit status, which the adapter reports as a failed run.
		if (plainSystem && !sentAsWritten) {
			process.stderr.write(
				"model_execution payload hook: the system text could not be sent as written\n",
			);
			process.exit(1);
		}
		const maxOutputTokens = requiredNumber(MAX_OUTPUT_TOKENS_ENV);
		const requestedTemperature = process.env[TEMPERATURE_ENV] === undefined
			? undefined : requiredNumber(TEMPERATURE_ENV);
		const refusesTemperature = modelRefusesTemperature(ctx);
		const temperature = refusesTemperature ? undefined : requestedTemperature;
		const payload = event.payload;
		if (!isPlainObject(payload)) {
			throw new Error(
				"model_execution payload hook: unsupported provider payload shape",
			);
		}
		// Closed shape set: at least one known provider-native output-token
		// spelling must already exist on the payload (or a Gemini-style
		// generationConfig object). Anything else is an unknown wire family
		// and the request fails closed rather than running uncapped. Every
		// recognized spelling that is present gets rewritten — an earlier
		// extension can leave more than one (say `max_completion_tokens`
		// plus `max_tokens`), and rewriting only the first would preserve a
		// larger limit in the field the provider actually honors.
		const spellings = [
			"max_output_tokens",
			"max_completion_tokens",
			"max_tokens",
			"maxOutputTokens",
		].filter((name) => name in payload);
		// A present but non-object generationConfig would be copied through unowned; the hook
		// refuses rather than forward a generation-control field it cannot cap.
		for (const name of ["generationConfig", "inferenceConfig"]) {
			if (name in payload && !isPlainObject(payload[name])) {
				throw new Error(`model_execution payload hook: unsupported ${name} shape`);
			}
		}
		const generationConfig = isPlainObject(payload.generationConfig)
			? payload.generationConfig
			: null;
		const inferenceConfig = isPlainObject(payload.inferenceConfig)
			? payload.inferenceConfig
			: null;
		if (spellings.length === 0 && generationConfig === null && inferenceConfig === null) {
			throw new Error(
				"model_execution payload hook: no recognized output-token field; refusing to send without generation controls",
			);
		}
		const next = { ...payload };
		if (Array.isArray(next.messages)) {
			next.messages = withoutConversationCacheMarkers(next.messages);
		}
		for (const name of spellings) {
			next[name] = maxOutputTokens;
		}
		if (spellings.length > 0 && temperature !== undefined) {
			next.temperature = temperature;
		}
		if (refusesTemperature) {
			delete next.temperature;
		}
		if (generationConfig !== null) {
			next.generationConfig = {
				...generationConfig,
				maxOutputTokens,
				...(temperature === undefined ? {} : { temperature }),
			};
			if (refusesTemperature) {
				delete next.generationConfig.temperature;
			}
		}
		if (inferenceConfig !== null) {
			next.inferenceConfig = {
				...inferenceConfig,
				maxTokens: maxOutputTokens,
				...(temperature === undefined ? {} : { temperature }),
			};
			if (refusesTemperature) {
				delete next.inferenceConfig.temperature;
			}
		}
		return next;
	});
}
