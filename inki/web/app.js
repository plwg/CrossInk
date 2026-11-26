// inki web app: upload an Anki export, download deck files.
// Everything is processed by the local inki server; nothing leaves this computer.

const $ = (id) => document.getElementById(id);

/** Creates an element. Text is always set with textContent, never as HTML. */
function el(tag, props = {}, ...children) {
	const node = document.createElement(tag);
	for (const [key, value] of Object.entries(props)) {
		if (key === "className") node.className = value;
		else if (key === "text") node.textContent = value;
		else node.setAttribute(key, value);
	}
	for (const child of children.flat()) {
		if (child == null || child === false) continue;
		node.append(
			child instanceof Node ? child : document.createTextNode(String(child)),
		);
	}
	return node;
}

async function request(path, body) {
	const options = { method: "POST" };
	if (body instanceof FormData) {
		options.body = body;
	} else {
		options.headers = { "Content-Type": "application/json" };
		options.body = JSON.stringify(body);
	}
	const response = await fetch(path, options);
	const data = await response.json().catch(() => ({}));
	if (!response.ok)
		throw new Error(data.error || `Request failed (${response.status})`);
	return data;
}

/** Starts the browser download of a file the server prepared. */
function save({ url, filename }) {
	const link = el("a", { href: url, download: filename });
	document.body.append(link);
	link.click();
	link.remove();
}

function notify(message, kind = "good") {
	const notice = $("notice");
	notice.textContent = message;
	notice.className = `notice ${kind}`;
	notice.hidden = false;
}

/** Runs an action with the button showing it is busy; reports errors. */
async function withBusy(button, action) {
	const label = button.textContent;
	button.disabled = true;
	button.classList.add("busy");
	$("notice").hidden = true;
	try {
		return await action();
	} catch (error) {
		notify(error.message, "error");
		return undefined;
	} finally {
		button.disabled = false;
		button.classList.remove("busy");
		button.textContent = label;
		refreshButtons();
	}
}

const plural = (n, one, many = `${one}s`) =>
	`${n.toLocaleString()} ${n === 1 ? one : many}`;
const percent = (x) => `${Math.round(x * 100)}%`;

const state = { anki: null, training: null, deck: null };
const MAX_PER_DAY = 9999;

/** Number inputs for the daily limits and a slider for desired retention. */
function settingsFields(container, prefix, values) {
	const number = (key, value, label, hint) =>
		el(
			"label",
			{},
			el("span", { className: "label", text: label }),
			el("input", {
				id: `${prefix}-${key}`,
				type: "number",
				min: "0",
				max: String(MAX_PER_DAY),
				step: "1",
				value: String(value),
			}),
			el("span", { className: "hint", text: hint }),
		);
	const retention = values.desired_retention;
	const slider = el("input", {
		id: `${prefix}-retention`,
		type: "range",
		min: String(Math.min(0.7, retention)),
		max: String(Math.max(0.99, retention)),
		// A retention set more finely elsewhere (e.g. 0.875) is not rounded.
		step: Math.round(retention * 100) / 100 === retention ? "0.01" : "any",
		value: String(retention),
	});
	const shown = el("output", { text: percent(retention) });
	slider.addEventListener("input", () => {
		shown.textContent = percent(Number(slider.value));
	});
	container.replaceChildren(
		number(
			"new",
			values.new_per_day,
			"New cards per day",
			"Cards seen for the first time.",
		),
		number(
			"reviews",
			values.reviews_per_day,
			"Reviews per day",
			"Cards you forget again are not limited.",
		),
		el(
			"label",
			{},
			el("span", { className: "label", text: "Desired retention" }),
			el("span", { className: "retention-row" }, slider, shown),
			el("span", {
				className: "hint",
				text: "The chance of remembering a card when it comes due. Higher means more reviews.",
			}),
		),
	);
}

function readSettings(prefix) {
	const limit = (key, name) => {
		const text = $(`${prefix}-${key}`).value.trim();
		const value = Number(text);
		if (
			text === "" ||
			!Number.isInteger(value) ||
			value < 0 ||
			value > MAX_PER_DAY
		)
			throw new Error(
				`${name} must be a whole number from 0 to ${MAX_PER_DAY}.`,
			);
		return value;
	};
	return {
		new_per_day: limit("new", "New cards per day"),
		reviews_per_day: limit("reviews", "Reviews per day"),
		desired_retention: Number($(`${prefix}-retention`).value),
	};
}

const describeSettings = (s) =>
	`${plural(s.new_per_day, "new card")} and ${plural(s.reviews_per_day, "review")} a day, ` +
	`${percent(s.desired_retention)} retention, ${s.trained_params ? "trained" : "default"} parameters`;

function refreshButtons() {
	$("update").disabled = !state.deck;
	$("train").disabled = !state.anki || state.anki.reviews === 0;
	$("add-cards").disabled = !state.anki;
	$("add-cards-label").textContent = state.anki
		? "Add new cards from my Anki export"
		: "Add new cards (choose your Anki export in step 1)";
}

/** Parameter choices for the deck being updated. */
function refreshParamsChoice() {
	if (!state.deck) return;
	const trained = state.deck.settings.trained_params;
	const options = [
		["keep", `Keep the deck's ${trained ? "trained" : "default"} parameters`],
	];
	if (trained) options.push(["default", "Go back to the FSRS-7 defaults"]);
	if (state.training)
		options.push(["trained", "Use my parameters trained in step 2"]);
	$("update-params").replaceChildren(
		...options.map(([value, text]) => el("option", { value, text })),
	);
}

function table(headers, rows) {
	return el(
		"table",
		{},
		el(
			"thead",
			{},
			el(
				"tr",
				{},
				headers.map(([text, num]) =>
					el("th", { className: num ? "num" : "", text }),
				),
			),
		),
		el("tbody", {}, rows),
	);
}

async function downloadDeck(button, name) {
	const ready = await withBusy(button, () =>
		request("/api/deck", {
			anki: state.anki.id,
			deck: name,
			settings: readSettings("new"),
			params:
				state.training && $("use-trained").checked ? "trained" : "default",
		}),
	);
	if (!ready) return;
	save(ready);
	const s = ready.summary;
	notify(
		`${ready.filename}: ${plural(s.cards, "card")}, ${s.studied.toLocaleString()} already studied` +
			(s.suspended
				? `, ${plural(s.suspended, "suspended card")} left out`
				: "") +
			`; ${describeSettings(s.settings)}. Copy it into the flashcards folder on the SD card.`,
	);
}

function renderDecks() {
	const decks = state.anki.decks;
	if (decks.length === 0) {
		$("decks").replaceChildren(
			el("p", { className: "empty", text: "This export has no cards." }),
		);
		return;
	}
	const rows = decks.map((deck) => {
		const depth = deck.name.split("::").length - 1;
		const button = el("button", {
			type: "button",
			className: "small",
			text: "Download",
		});
		button.addEventListener("click", () => downloadDeck(button, deck.name));
		return el(
			"tr",
			{},
			el("td", {
				style: `padding-left: ${8 + depth * 18}px`,
				text: deck.name.split("::").at(-1),
			}),
			el("td", { className: "num", text: deck.cards.toLocaleString() }),
			el("td", { className: "num", text: deck.new.toLocaleString() }),
			el("td", { className: "num", text: deck.suspended.toLocaleString() }),
			el("td", { className: "num" }, button),
		);
	});
	$("decks").replaceChildren(
		table(
			[
				["Deck"],
				["Cards", true],
				["New", true],
				["Suspended", true],
				["", true],
			],
			rows,
		),
	);
}

function renderTraining(result) {
	const fmt = (x) => x.toFixed(4);
	const better = result.trained.log_loss < result.defaults.log_loss;
	const stat = (name, fit) =>
		el(
			"div",
			{ className: "stat" },
			el("div", { className: "name", text: name }),
			el("div", { className: "value", text: fmt(fit.log_loss) }),
			el("div", {
				className: "name",
				text: `log loss · RMSE ${fmt(fit.rmse_bins)}`,
			}),
		);
	$("train-result").replaceChildren(
		el(
			"div",
			{ className: "result" },
			el("p", {
				text: `Trained on ${plural(result.reviews, "review")} of ${plural(result.cards, "card")}. Lower is better.`,
			}),
			el(
				"div",
				{ className: "compare" },
				stat("Default parameters", result.defaults),
				stat("Your parameters", result.trained),
			),
			el("p", {
				className: "hint",
				text: better
					? "Your parameters fit your history better."
					: "The defaults fit about as well as your own parameters.",
			}),
		),
	);
	$("use-trained").checked = better;
	$("use-trained-row").hidden = false;
	$("params-state").textContent = "Trained";
	$("params-state").className = "pill good";
	refreshParamsChoice();
}

$("anki-file").addEventListener("change", async () => {
	const file = $("anki-file").files[0];
	if (!file) return;
	$("anki-name").textContent = file.name;
	state.anki = null;
	state.training = null;
	$("train-result").replaceChildren();
	$("use-trained-row").hidden = true;
	$("params-state").textContent = "";
	refreshParamsChoice();
	$("anki-state").textContent = "Reading…";
	$("anki-state").className = "pill";
	$("decks").replaceChildren(
		el("p", { className: "empty", text: "Reading your export…" }),
	);
	refreshButtons();
	const form = new FormData();
	form.append("file", file);
	try {
		state.anki = await request("/api/anki", form);
	} catch (error) {
		$("anki-state").textContent = "";
		$("decks").replaceChildren(
			el("p", { className: "empty", text: "Choose your Anki export first." }),
		);
		notify(error.message, "error");
		return;
	}
	$("notice").hidden = true;
	const reviews = state.anki.reviews;
	$("anki-state").textContent = reviews
		? `${plural(state.anki.decks.length, "deck")} · ${plural(reviews, "review")}`
		: `${plural(state.anki.decks.length, "deck")} · no review history`;
	$("anki-state").className = "pill good";
	renderDecks();
	refreshButtons();
});

$("deck-file").addEventListener("change", async () => {
	const file = $("deck-file").files[0];
	$("deck-name").textContent = file ? file.name : "No file chosen";
	$("update-result").replaceChildren();
	$("update-form").hidden = true;
	state.deck = null;
	refreshButtons();
	if (!file) return;
	const form = new FormData();
	form.append("deck", file);
	try {
		state.deck = await request("/api/inspect", form);
	} catch (error) {
		notify(error.message, "error");
		return;
	}
	$("notice").hidden = true;
	const d = state.deck;
	$("deck-info").textContent =
		`“${d.anki_deck}”: ${plural(d.cards, "card")}, ${d.studied.toLocaleString()} studied, ` +
		`${plural(d.answers, "answer")} given on the reader.`;
	settingsFields($("update-settings"), "update", d.settings);
	refreshParamsChoice();
	$("update-form").hidden = false;
	refreshButtons();
});

$("update").addEventListener("click", async () => {
	const ready = await withBusy($("update"), () => {
		const form = new FormData();
		form.append("deck", $("deck-file").files[0]);
		form.append("settings", JSON.stringify(readSettings("update")));
		form.append("params", $("update-params").value);
		if (state.anki && $("add-cards").checked)
			form.append("anki", String(state.anki.id));
		return request("/api/update", form);
	});
	if (!ready) return;
	save(ready);
	const s = ready.summary;
	const parts = [];
	const a = s.from_anki;
	if (a) {
		parts.push(`${plural(a.new_cards, "new card")} added`);
		parts.push(
			`${a.studied_on_reader.toLocaleString()} studied on the reader, progress kept`,
		);
		if (a.text_changes)
			parts.push(`${plural(a.text_changes, "card")} with edited text`);
		if (a.missing_in_anki)
			parts.push(
				`${plural(a.missing_in_anki, "card")} deleted in Anki, kept on the reader`,
			);
	}
	parts.push(describeSettings(s.settings));
	$("update-result").replaceChildren(
		el(
			"div",
			{ className: "result" },
			el("p", {
				text: `${ready.filename} from “${s.anki_deck}”: ${plural(s.cards, "card")}.`,
			}),
			el("p", {
				className: "hint",
				text: `${parts.join(" · ")}. Copy it back over the old file.`,
			}),
		),
	);
});

$("train").addEventListener("click", async () => {
	const result = await withBusy($("train"), () =>
		request("/api/train", { anki: state.anki.id }),
	);
	if (result) {
		state.training = result;
		renderTraining(result);
	}
});

settingsFields($("new-settings"), "new", {
	new_per_day: 20,
	reviews_per_day: 200,
	desired_retention: 0.9,
});
refreshButtons();
