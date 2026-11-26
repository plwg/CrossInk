#include "FlashcardDeckListActivity.h"

#include <GfxRenderer.h>
#include <I18n.h>
#include <Logging.h>
#include <Memory.h>

#include <cstdio>

#include "FlashcardReviewActivity.h"
#include "MappedInputManager.h"
#include "components/CompactHeader.h"
#include "components/TouchHeaderBackButton.h"
#include "components/UITheme.h"
#include "components/UIThemeTokens.h"
#include "components/UiAppHelpers.h"
#include "flashcards/FlashcardStore.h"

namespace fui = freeink::ui;

namespace {
constexpr fui::ActionId ACTION_ROW = 1;
}  // namespace

FlashcardDeckListActivity::FlashcardDeckListActivity(GfxRenderer& renderer, MappedInputManager& mappedInput)
    : Activity("FlashcardDecks", renderer, mappedInput),
      uiTarget(makeUiTarget(renderer)),
      app(uiTarget, uiTarget.deviceContext()) {}

void FlashcardDeckListActivity::reloadDecks() {
  decks.clear();
  std::vector<std::string> names;
  flashcards::listDecks(names);

  uint32_t now = 0;
  hasClock = flashcards::currentTime(now);
  const flashcards::StudyDay day = flashcards::studyDayFor(now, flashcards::utcOffsetSeconds());

  decks.reserve(names.size());
  flashcards::ReviewSession session;
  for (auto& name : names) {
    DeckRow row{std::move(name), {}};
    flashcards::Deck deck;
    uint16_t newDone = 0;
    uint16_t reviewsDone = 0;
    if (!deck.open(row.name)) {
      row.counts = deck.needsNewerFirmware() ? tr(STR_FLASHCARDS_DECK_TOO_NEW) : tr(STR_FLASHCARDS_DECK_FAILED);
    } else if (hasClock) {
      deck.countToday(day, newDone, reviewsDone);
      if (session.begin(deck.config(), deck.limits(), day, newDone, reviewsDone) && deck.loadSession(session)) {
        char buf[64];
        snprintf(buf, sizeof(buf), tr(STR_FLASHCARDS_DECK_COUNTS), static_cast<unsigned>(session.dueCount()),
                 static_cast<unsigned>(session.newCount()));
        row.counts = buf;
      } else {
        row.counts = tr(STR_FLASHCARDS_DECK_FAILED);
      }
      session.clear();
    }
    decks.push_back(std::move(row));
  }

  if (decks.empty()) {
    selectedIndex = 0;
  } else if (selectedIndex >= static_cast<int>(decks.size())) {
    selectedIndex = static_cast<int>(decks.size()) - 1;
  }
}

void FlashcardDeckListActivity::onEnter() {
  Activity::onEnter();
  selectedIndex = 0;
  topIndex = 0;
  visibleRows = 1;
  reloadDecks();
  uiReady = false;
  applySharedUiTheme(app, uiTarget);
  app.on(ACTION_ROW, &FlashcardDeckListActivity::onRowEvent, this);
  app.setScreen(&FlashcardDeckListActivity::listScreen, this);
  requestUpdate();
}

void FlashcardDeckListActivity::onExit() {
  decks.clear();
  Activity::onExit();
}

void FlashcardDeckListActivity::openDeck(const int index) {
  if (!hasClock || index < 0 || index >= static_cast<int>(decks.size())) return;
  auto review = makeUniqueNoThrow<FlashcardReviewActivity>(renderer, mappedInput, decks[index].name);
  if (!review) {
    LOG_ERR("FLC", "OOM: FlashcardReviewActivity");
    return;
  }
  startActivityForResult(std::move(review), [this](const ActivityResult&) {
    reloadDecks();
    requestUpdate();
  });
}

void FlashcardDeckListActivity::loop() {
  if (TouchHeaderBackButton::wasTapped(mappedInput, TouchHeaderBackButton::compactHeaderRect(renderer)) ||
      mappedInput.wasReleased(MappedInputManager::Button::Back)) {
    finish();
    return;
  }
  if (mappedInput.wasReleased(MappedInputManager::Button::Confirm)) {
    openDeck(selectedIndex);
    return;
  }

  const int listSize = static_cast<int>(decks.size());
  if (listSize == 0) return;

  if (uiReady) {
    const fui::InputSnapshot snap = touchSnapshotFrom(mappedInput);
    if (snap.touchPressed || snap.touchReleased) {
      const auto event = app.route(snap);
      if (app.invalidated()) requestUpdate();
      if (event) return;
    }
  }

  const auto moveSelection = [this, listSize](const int next) {
    selectedIndex = next;
    topIndex = followListSelection(selectedIndex, topIndex, visibleRows, listSize);
    requestUpdate();
  };
  buttonNavigator.onNextRelease(
      [this, listSize, &moveSelection] { moveSelection(ButtonNavigator::nextIndex(selectedIndex, listSize)); });
  buttonNavigator.onPreviousRelease(
      [this, listSize, &moveSelection] { moveSelection(ButtonNavigator::previousIndex(selectedIndex, listSize)); });
  buttonNavigator.onNextContinuous([this, listSize, &moveSelection] {
    moveSelection(ButtonNavigator::nextPageIndex(selectedIndex, listSize, visibleRows));
  });
  buttonNavigator.onPreviousContinuous([this, listSize, &moveSelection] {
    moveSelection(ButtonNavigator::previousPageIndex(selectedIndex, listSize, visibleRows));
  });
}

void FlashcardDeckListActivity::onRowEvent(const fui::ActionEvent& event, void* user) {
  auto* self = static_cast<FlashcardDeckListActivity*>(user);
  if (event.value < 0 || event.value >= static_cast<int16_t>(self->decks.size())) return;
  self->selectedIndex = event.value;
  self->app.clearTapFlash();
  self->openDeck(self->selectedIndex);
}

void FlashcardDeckListActivity::listScreen(UiApp::ScreenType& screen, void* user) {
  static_cast<FlashcardDeckListActivity*>(user)->buildListScreen(screen);
}

void FlashcardDeckListActivity::buildListScreen(UiApp::ScreenType& screen) {
  const auto& metrics = UITheme::getInstance().getMetrics();
  screen.setContentMargin(fui::Insets{static_cast<int16_t>(CompactHeader::contentTop(metrics)), 0,
                                      static_cast<int16_t>(metrics.buttonHintsHeight), 0});
  screen.spacer(static_cast<int16_t>(metrics.verticalSpacing));
  if (!hasClock) {
    screen.centeredText(tr(STR_FLASHCARDS_NEEDS_CLOCK), screen.theme().bodyText);
    return;
  }
  if (decks.empty()) {
    screen.centeredText(tr(STR_FLASHCARDS_NO_DECKS), screen.theme().bodyText);
    return;
  }
  std::vector<fui::ListItem> items;
  items.reserve(decks.size());
  for (size_t i = 0; i < decks.size(); ++i) {
    fui::ListItem item;
    item.label = decks[i].name.c_str();
    if (!decks[i].counts.empty()) item.subtitle = decks[i].counts.c_str();
    item.actionValue = static_cast<int16_t>(i);
    items.push_back(item);
  }
  fui::ListProps props;
  props.items = items.data();
  props.count = static_cast<uint16_t>(items.size());
  props.selectedIndex = static_cast<int16_t>(selectedIndex);
  props.action = ACTION_ROW;
  props.inputMask = static_cast<uint16_t>(fui::InputTouch);
  props.labelText = screen.theme().bodyText;
  props.labelText.bold = true;
  const fui::Rect bounds = screen.body();
  const auto rows = configureUiList(props, screen.theme(), bounds, UiListRowType::WithSubtitle);
  visibleRows = rows > 0 ? rows : 1;
  topIndex = scrollListBy(topIndex, 0, visibleRows, static_cast<int>(decks.size()));
  props.topIndex = static_cast<uint16_t>(topIndex);
  screen.list(props);
}

void FlashcardDeckListActivity::render(RenderLock&&) {
  renderer.clearScreen();
  if (mappedInput.hasTouchHardware()) {
    TouchHeaderBackButton::drawCompact(renderer, tr(STR_FLASHCARDS));
  } else {
    CompactHeader::drawTitle(renderer, tr(STR_FLASHCARDS));
  }
  uiReady = false;
  app.render();
  uiReady = true;

  const bool canOpen = hasClock && !decks.empty();
  const auto labels =
      mappedInput.mapLabels(mappedInput.withBackArrow(tr(STR_HOME)), canOpen ? tr(STR_FLASHCARDS_STUDY) : "",
                            canOpen ? tr(STR_DIR_UP) : "", canOpen ? tr(STR_DIR_DOWN) : "");
  GUI.drawButtonHints(renderer, labels.btn1, labels.btn2, labels.btn3, labels.btn4);
  renderer.displayBuffer();
}
