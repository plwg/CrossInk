# inki-core (Rust) provides FSRS-7 and the .inki format; build its C library
# for the host. The inki repository is expected next to this one, or at
# $INKI_DIR.
if(DEFINED ENV{INKI_DIR})
  set(INKI_DIR "$ENV{INKI_DIR}")
else()
  get_filename_component(INKI_DIR "${REPO_ROOT}/../inki" ABSOLUTE)
endif()
set(INKI_FFI_DIR "${INKI_DIR}/crates/inki-ffi")
if(NOT EXISTS "${INKI_FFI_DIR}/Cargo.toml")
  message(FATAL_ERROR "inki-ffi not found in ${INKI_DIR}; clone inki next to this repository or set INKI_DIR")
endif()
find_program(CARGO cargo HINTS "$ENV{HOME}/.cargo/bin" REQUIRED)
set(INKI_FFI_TARGET_DIR "${CMAKE_CURRENT_BINARY_DIR}/inki-ffi")
set(INKI_FFI_LIB "${INKI_FFI_TARGET_DIR}/release/libinki_ffi.a")
add_custom_target(inki_ffi_build
  COMMAND "${CARGO}" build --release --quiet
          --manifest-path "${INKI_FFI_DIR}/Cargo.toml"
          --target-dir "${INKI_FFI_TARGET_DIR}"
  BYPRODUCTS "${INKI_FFI_LIB}"
  COMMENT "Building inki-core for the host"
)
add_library(inki_ffi STATIC IMPORTED)
set_target_properties(inki_ffi PROPERTIES IMPORTED_LOCATION "${INKI_FFI_LIB}")
add_dependencies(inki_ffi inki_ffi_build)

add_executable(FlashcardsTest
  FlashcardsTest.cpp
  ${REPO_ROOT}/lib/Flashcards/FlashcardFormat.cpp
  ${REPO_ROOT}/lib/Flashcards/ReviewSession.cpp
)

target_include_directories(FlashcardsTest PRIVATE
  ${REPO_ROOT}/lib/Flashcards
  ${REPO_ROOT}/lib/Fsrs7
  ${REPO_ROOT}/lib/Memory
  ${INKI_FFI_DIR}/include
)

target_link_libraries(FlashcardsTest PRIVATE
  crosspoint_test_common
  GTest::gtest_main
  inki_ffi
)

gtest_discover_tests(FlashcardsTest)
