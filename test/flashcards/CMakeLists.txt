add_executable(FlashcardsTest
  FlashcardsTest.cpp
  ${REPO_ROOT}/lib/Flashcards/FlashcardFormat.cpp
  ${REPO_ROOT}/lib/Flashcards/ReviewSession.cpp
  ${REPO_ROOT}/lib/Fsrs7/Fsrs7.cpp
)

target_include_directories(FlashcardsTest PRIVATE
  ${REPO_ROOT}/lib/Flashcards
  ${REPO_ROOT}/lib/Fsrs7
  ${REPO_ROOT}/lib/Memory
)

target_link_libraries(FlashcardsTest PRIVATE
  crosspoint_test_common
  GTest::gtest_main
)

gtest_discover_tests(FlashcardsTest)
