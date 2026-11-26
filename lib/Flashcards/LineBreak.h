#pragma once

// Line breaking for flashcard text, which may mix English and Chinese.
// Lines break at spaces and between CJK characters (which use no spaces),
// keeping CJK closing punctuation (，。！？」…) off the start of a line and
// opening punctuation (「（《…) off the end. A word wider than the line is
// split between characters. Pure logic, unit-tested on the host.

#include <cstddef>
#include <cstdint>

namespace flashcards {

// Width in pixels of text[0, length). `context` is passed through.
using MeasureText = int (*)(void* context, const char* text, size_t length);

struct LineSpan {
  size_t length;  // bytes of text on this line (trailing spaces dropped)
  size_t next;    // where the next line starts (spaces at the break skipped)
};

// The first line of `text` (no newlines) that fits in `maxWidth`. At least
// one character is placed, so the caller always makes progress.
LineSpan breakLine(const char* text, size_t length, int maxWidth, MeasureText measure, void* context);

// True for Han, kana, Hangul and full-width forms: text without spaces that
// may break between any two characters.
bool isCjk(uint32_t codepoint);

}  // namespace flashcards
