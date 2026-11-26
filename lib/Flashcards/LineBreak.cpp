#include "LineBreak.h"

namespace flashcards {

namespace {

// Decodes the UTF-8 character at text[pos]; sets `size` to its byte length.
// Invalid bytes decode as themselves, one byte each.
uint32_t decodeAt(const char* text, size_t length, size_t pos, size_t& size) {
  const auto byte = [&](size_t i) { return static_cast<uint8_t>(text[i]); };
  const uint8_t lead = byte(pos);
  size = 1;
  size_t need = 0;
  uint32_t cp = 0;
  if ((lead & 0xE0) == 0xC0) {
    need = 1;
    cp = lead & 0x1F;
  } else if ((lead & 0xF0) == 0xE0) {
    need = 2;
    cp = lead & 0x0F;
  } else if ((lead & 0xF8) == 0xF0) {
    need = 3;
    cp = lead & 0x07;
  } else {
    return lead;  // ASCII or an invalid lead byte
  }
  for (size_t i = 1; i <= need; ++i) {
    if (pos + i >= length || (byte(pos + i) & 0xC0) != 0x80) return lead;
    cp = (cp << 6) | (byte(pos + i) & 0x3F);
  }
  size = need + 1;
  return cp;
}

// Must not start a line: closing CJK punctuation and ASCII closers.
bool noLineStart(uint32_t cp) {
  switch (cp) {
    case 0x3001:  // 、
    case 0x3002:  // 。
    case 0xFF0C:  // ，
    case 0xFF0E:  // ．
    case 0xFF1A:  // ：
    case 0xFF1B:  // ；
    case 0xFF01:  // ！
    case 0xFF1F:  // ？
    case 0xFF09:  // ）
    case 0xFF3D:  // ］
    case 0xFF5D:  // ｝
    case 0x3009:  // 〉
    case 0x300B:  // 》
    case 0x300D:  // 」
    case 0x300F:  // 』
    case 0x3011:  // 】
    case 0x3015:  // 〕
    case 0x3017:  // 〗
    case 0x30FB:  // ・
    case 0x30FC:  // ー
    case 0x2026:  // …
    case 0x2025:  // ‥
    case 0x201D:  // ”
    case 0x2019:  // ’
    case 0x00B7:  // ·
    case ')':
    case ']':
    case '}':
    case ',':
    case '.':
    case ';':
    case ':':
    case '!':
    case '?':
    case '%':
      return true;
    default:
      return false;
  }
}

// Must not end a line: opening CJK punctuation.
bool noLineEnd(uint32_t cp) {
  switch (cp) {
    case 0xFF08:  // （
    case 0xFF3B:  // ［
    case 0xFF5B:  // ｛
    case 0x3008:  // 〈
    case 0x300A:  // 《
    case 0x300C:  // 「
    case 0x300E:  // 『
    case 0x3010:  // 【
    case 0x3014:  // 〔
    case 0x3016:  // 〖
    case 0x201C:  // “
    case 0x2018:  // ‘
      return true;
    default:
      return false;
  }
}

// End of the unbreakable piece starting at `pos` (not a space): one CJK
// character, or a run of other characters; plus any closing punctuation
// that follows, and the next piece after opening punctuation.
size_t tokenEnd(const char* text, size_t length, size_t pos) {
  size_t size = 0;
  uint32_t cp = decodeAt(text, length, pos, size);
  // Opening punctuation sticks to what follows.
  while (noLineEnd(cp) && pos + size < length && text[pos + size] != ' ') {
    pos += size;
    cp = decodeAt(text, length, pos, size);
  }
  size_t end = pos + size;
  if (!isCjk(cp)) {
    while (end < length && text[end] != ' ') {
      const uint32_t next = decodeAt(text, length, end, size);
      if (isCjk(next) || noLineEnd(next)) break;
      end += size;
    }
  }
  while (end < length) {
    const uint32_t next = decodeAt(text, length, end, size);
    if (!noLineStart(next)) break;
    end += size;
  }
  return end;
}

}  // namespace

bool isCjk(uint32_t cp) {
  return (cp >= 0x1100 && cp <= 0x11FF) ||  // Hangul Jamo
         (cp >= 0x2E80 && cp <= 0x303F) ||  // CJK radicals, symbols and punctuation
         (cp >= 0x3040 && cp <= 0x9FFF) ||  // kana, bopomofo, CJK ideographs
         (cp >= 0xAC00 && cp <= 0xD7AF) ||  // Hangul syllables
         (cp >= 0xF900 && cp <= 0xFAFF) ||  // CJK compatibility ideographs
         (cp >= 0xFE30 && cp <= 0xFE4F) ||  // CJK compatibility forms
         (cp >= 0xFF00 && cp <= 0xFFEF) ||  // full-width forms
         (cp >= 0x20000 && cp <= 0x3FFFF);  // CJK extensions
}

LineSpan breakLine(const char* text, size_t length, int maxWidth, MeasureText measure, void* context) {
  const int spaceWidth = measure(context, " ", 1);
  int width = 0;
  size_t lineEnd = 0;     // end of the content placed so far
  bool placed = false;    // anything but spaces placed
  int pendingSpaces = 0;  // width of spaces before the next piece
  size_t pos = 0;
  while (pos < length) {
    if (text[pos] == ' ') {
      while (pos < length && text[pos] == ' ') {
        pendingSpaces += spaceWidth;
        ++pos;
      }
      continue;
    }
    const size_t end = tokenEnd(text, length, pos);
    const int tokenWidth = measure(context, text + pos, end - pos);
    if (width + pendingSpaces + tokenWidth <= maxWidth) {
      width += pendingSpaces + tokenWidth;
      pendingSpaces = 0;
      lineEnd = end;
      placed = true;
      pos = end;
      continue;
    }
    if (placed) return LineSpan{lineEnd, pos};
    // The piece alone is too wide: place as many characters as fit, at least one.
    size_t cut = pos;
    size_t size = 0;
    decodeAt(text, length, cut, size);
    cut += size;
    while (cut < end) {
      decodeAt(text, length, cut, size);
      if (width + pendingSpaces + measure(context, text + pos, cut + size - pos) > maxWidth) break;
      cut += size;
    }
    return LineSpan{cut, cut};
  }
  return LineSpan{placed ? lineEnd : 0, length};
}

}  // namespace flashcards
