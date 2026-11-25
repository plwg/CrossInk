#include "FlashcardStore.h"

#include <HalClock.h>
#include <HalStorage.h>
#include <Logging.h>
#include <Memory.h>
#include <strings.h>

#include <algorithm>
#include <cstring>
#include <ctime>

#include "CrossPointSettings.h"

namespace flashcards {

namespace {

constexpr char TAG[] = "FLC";
// 2024-01-01 00:00:00 UTC. Clocks before this have never been set.
constexpr uint32_t kEarliestValidTime = 1704067200;
// State records read per step when loading a session.
constexpr size_t kStateChunkRecords = 8;
// Answer-log entries read per step when counting today's answers.
constexpr size_t kLogChunkEntries = 4;

// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's algorithm).
int64_t daysFromCivil(int64_t y, unsigned m, unsigned d) {
  y -= m <= 2;
  const int64_t era = (y >= 0 ? y : y - 399) / 400;
  const unsigned yoe = static_cast<unsigned>(y - era * 400);
  const unsigned doy = (153 * (m > 2 ? m - 3 : m + 9) + 2) / 5 + d - 1;
  const unsigned doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
  return era * 146097 + static_cast<int64_t>(doe) - 719468;
}

bool readExact(FsFile& file, void* buf, size_t length) { return file.read(buf, length) == static_cast<int>(length); }

}  // namespace

bool hasFlashcardsFolder() { return Storage.exists(kRootDir); }

static bool endsWith(const char* name, const char* suffix) {
  const size_t n = strlen(name);
  const size_t m = strlen(suffix);
  return n > m && strcasecmp(name + n - m, suffix) == 0;
}

void listDecks(std::vector<std::string>& out) {
  out.clear();
  if (!Storage.exists(kRootDir)) return;
  for (const auto& file : Storage.listFiles(kRootDir)) {
    const char* name = file.c_str();
    // Skip hidden files, including macOS "._" copies on FAT cards.
    if (name[0] == '.' || !endsWith(name, kDeckExtension)) continue;
    out.emplace_back(name, strlen(name) - strlen(kDeckExtension));
  }
  std::sort(out.begin(), out.end());
}

fsrs7::Config loadConfig() {
  fsrs7::Config config = fsrs7::defaultConfig();
  if (!Storage.exists(kParamsPath)) return config;
  // The file is about 400 bytes; 1 KB leaves room for comments. Heap, not
  // stack: it is only needed once per deck open.
  constexpr size_t kMaxConfigBytes = 1024;
  auto text = makeUniqueNoThrow<char[]>(kMaxConfigBytes);
  if (!text) {
    LOG_ERR(TAG, "OOM: %u bytes for %s; using FSRS-7 defaults", static_cast<unsigned>(kMaxConfigBytes), kParamsPath);
    return config;
  }
  const size_t length = Storage.readFileToBuffer(kParamsPath, text.get(), kMaxConfigBytes);
  if (length == 0 || !fsrs7::parseConfig(text.get(), length, config)) {
    LOG_ERR(TAG, "Ignoring invalid %s; using FSRS-7 defaults", kParamsPath);
    return fsrs7::defaultConfig();
  }
  LOG_INF(TAG, "Loaded FSRS-7 parameters, desired retention %.2f", static_cast<double>(config.desiredRetention));
  return config;
}

bool currentTime(uint32_t& now) {
  uint16_t year = 0;
  uint8_t month = 0, day = 0, hour = 0, minute = 0;
  if (halClock.isAvailable() && halClock.getDateTime(year, month, day, hour, minute) && year >= 2024) {
    const int64_t seconds = daysFromCivil(year, month, day) * 86400 + hour * 3600 + minute * 60;
    if (seconds >= kEarliestValidTime && seconds <= UINT32_MAX) {
      now = static_cast<uint32_t>(seconds);
      return true;
    }
  }
  const time_t system = time(nullptr);
  if (system >= static_cast<time_t>(kEarliestValidTime)) {
    now = static_cast<uint32_t>(system);
    return true;
  }
  return false;
}

int32_t utcOffsetSeconds() { return (static_cast<int32_t>(SETTINGS.clockUtcOffsetQ) - 48) * 15 * 60; }

bool Deck::open(const std::string& name) {
  name_ = name;
  path_ = std::string(kRootDir) + "/" + name + kDeckExtension;
  header_ = DeckHeader{};

  FsFile file;
  if (!Storage.openFileForRead(TAG, path_, file)) {
    LOG_ERR(TAG, "Cannot open %s", path_.c_str());
    return false;
  }
  uint8_t raw[kDeckHeaderSize];
  const bool read = readExact(file, raw, sizeof(raw));
  const size_t size = file.fileSize();
  file.close();
  if (!read || !decodeDeckHeader(raw, size, header_)) {
    LOG_ERR(TAG, "%s is not a version %u deck file", path_.c_str(), kFormatVersion);
    header_ = DeckHeader{};
    return false;
  }
  return true;
}

void Deck::countToday(const StudyDay& day, uint16_t& newDone, uint16_t& reviewsDone) const {
  newDone = 0;
  reviewsDone = 0;
  FsFile file;
  if (!Storage.openFileForRead(TAG, path_, file)) return;
  size_t entries = reviewLogEntries(header_, file.fileSize());
  uint8_t chunk[kLogChunkEntries * kReviewLogEntrySize];
  bool reachedEarlierDay = false;
  // Walk backwards from the newest answer until one is before today.
  while (entries > 0 && !reachedEarlierDay) {
    const size_t n = std::min(entries, kLogChunkEntries);
    entries -= n;
    if (!file.seekSet(header_.logOffset + entries * kReviewLogEntrySize) ||
        !readExact(file, chunk, n * kReviewLogEntrySize)) {
      LOG_ERR(TAG, "Failed reading the answer log of %s", path_.c_str());
      break;
    }
    for (size_t i = n; i-- > 0;) {
      ReviewLogEntry entry{};
      decodeReviewLogEntry(chunk + i * kReviewLogEntrySize, entry);
      if (entry.reviewTime < day.start) {
        reachedEarlierDay = true;
        break;
      }
      if (entry.reviewTime >= day.end) continue;
      if (entry.flags & kReviewFlagWasNew) {
        if (newDone < UINT16_MAX) ++newDone;
      } else if (reviewsDone < UINT16_MAX) {
        ++reviewsDone;
      }
    }
  }
  file.close();
}

bool Deck::loadSession(ReviewSession& session) const {
  FsFile file;
  if (!Storage.openFileForRead(TAG, path_, file)) {
    LOG_ERR(TAG, "Cannot open %s", path_.c_str());
    return false;
  }
  uint8_t chunk[kStateChunkRecords * kStateRecordSize];
  bool ok = file.seekSet(header_.stateOffset);
  uint32_t index = 0;
  while (ok && index < header_.cardCount) {
    const uint32_t n = std::min<uint32_t>(header_.cardCount - index, kStateChunkRecords);
    ok = readExact(file, chunk, n * kStateRecordSize);
    for (uint32_t i = 0; ok && i < n; ++i) {
      StateRecord state{};
      decodeStateRecord(chunk + i * kStateRecordSize, state);
      session.consider(index + i, state);
    }
    index += n;
  }
  file.close();
  if (!ok) {
    LOG_ERR(TAG, "Failed reading card states of %s", path_.c_str());
    return false;
  }
  session.finishLoading();
  return true;
}

bool Deck::readCard(uint32_t index, char* buf, size_t bufSize, size_t& length, uint64_t& ankiCardId) const {
  length = 0;
  if (index >= header_.cardCount || bufSize == 0) return false;
  FsFile file;
  if (!Storage.openFileForRead(TAG, path_, file)) {
    LOG_ERR(TAG, "Cannot open %s", path_.c_str());
    return false;
  }
  uint8_t raw[kCardIndexEntrySize];
  CardIndexEntry entry{};
  bool ok = file.seekSet(header_.indexOffset + index * kCardIndexEntrySize) && readExact(file, raw, sizeof(raw));
  if (ok) {
    decodeCardIndexEntry(raw, entry);
    const size_t wanted = std::min<size_t>(entry.textLength, bufSize - 1);
    ok = static_cast<uint64_t>(entry.textOffset) + entry.textLength <= header_.stateOffset &&
         file.seekSet(entry.textOffset) && readExact(file, buf, wanted);
    if (ok) length = wanted;
  }
  file.close();
  buf[length] = '\0';
  if (!ok) {
    LOG_ERR(TAG, "Cannot read card %u of %s", index, name_.c_str());
    return false;
  }
  ankiCardId = entry.ankiCardId;
  return true;
}

bool Deck::saveAnswer(uint32_t index, const StateRecord& state, const ReviewLogEntry& log) const {
  if (index >= header_.cardCount) return false;
  FsFile file = Storage.open(path_.c_str(), O_RDWR);
  if (!file) {
    LOG_ERR(TAG, "Cannot open %s for writing", path_.c_str());
    return false;
  }
  uint8_t buf[kReviewLogEntrySize];
  // The log first: it is what the daily limits count. Appending after the
  // last complete entry overwrites any half-written one.
  encodeReviewLogEntry(log, buf);
  bool ok = file.seekSet(reviewLogAppendOffset(header_, file.fileSize())) &&
            file.write(buf, kReviewLogEntrySize) == kReviewLogEntrySize;
  if (ok) {
    encodeStateRecord(state, buf);
    ok = file.seekSet(header_.stateOffset + index * kStateRecordSize) &&
         file.write(buf, kStateRecordSize) == kStateRecordSize;
  }
  file.close();
  if (!ok) LOG_ERR(TAG, "Failed saving the answer for card %u in %s", index, path_.c_str());
  return ok;
}

}  // namespace flashcards
