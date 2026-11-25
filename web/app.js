// inki web app: upload an Anki export, download deck files and settings.
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

const state = { anki: null, training: null };

function refreshButtons() {
	$("update").disabled = !state.anki || $("deck-file").files.length === 0;
	$("train").disabled = !state.anki || state.anki.reviews === 0;
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
		request("/api/deck", { anki: state.anki.id, deck: name }),
	);
	if (!ready) return;
	save(ready);
	const s = ready.summary;
	notify(
		`${ready.filename}: ${plural(s.cards, "card")}, ${s.studied.toLocaleString()} already studied` +
			(s.suspended
				? `, ${plural(s.suspended, "suspended card")} left out`
				: "") +
			". Copy it into the flashcards folder on the SD card.",
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
					? "Your parameters fit your history better. Decks you download now use them too."
					: "The defaults fit about as well as your own parameters.",
			}),
		),
	);
	$("params-state").textContent = "Trained";
	$("params-state").className = "pill good";
}

$("anki-file").addEventListener("change", async () => {
	const file = $("anki-file").files[0];
	if (!file) return;
	$("anki-name").textContent = file.name;
	state.anki = null;
	state.training = null;
	$("train-result").replaceChildren();
	$("params-state").textContent = "";
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

$("deck-file").addEventListener("change", () => {
	const file = $("deck-file").files[0];
	$("deck-name").textContent = file ? file.name : "No file chosen";
	$("update-result").replaceChildren();
	refreshButtons();
});

$("update").addEventListener("click", async () => {
	const form = new FormData();
	form.append("anki", String(state.anki.id));
	form.append("deck", $("deck-file").files[0]);
	const ready = await withBusy($("update"), () => request("/api/update", form));
	if (!ready) return;
	save(ready);
	const s = ready.summary;
	const parts = [
		`${plural(s.new_cards, "new card")} added`,
		`${s.studied_on_reader.toLocaleString()} studied on the reader, progress kept`,
	];
	if (s.text_changes)
		parts.push(`${plural(s.text_changes, "card")} with edited text`);
	if (s.missing_in_anki)
		parts.push(
			`${plural(s.missing_in_anki, "card")} deleted in Anki, kept on the reader`,
		);
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

$("retention").addEventListener("input", () => {
	$("retention-value").textContent = percent(Number($("retention").value));
});

$("params-form").addEventListener("submit", async (event) => {
	event.preventDefault();
	const ready = await withBusy($("params-save"), () =>
		request("/api/params", {
			anki: state.training ? state.anki.id : null,
			desired_retention: Number($("retention").value),
		}),
	);
	if (!ready) return;
	save(ready);
	notify(
		`Saved ${ready.filename} (${ready.summary.trained ? "your parameters" : "default parameters"}, ` +
			`${percent(ready.summary.desired_retention)} retention). Copy it into the flashcards folder.`,
	);
});

refreshButtons();
