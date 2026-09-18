/** @file
 * @brief Owned byte buffers, strict UTF-8, and Unicode display properties.
 */
#pragma once
#include <stddef.h>
#include <stdint.h>

/** @brief Borrowed immutable bytes; nullptr is valid only for an empty span. */
typedef struct {
  const char *data; ///< Borrowed storage, not necessarily NUL-terminated.
  size_t size;      ///< Number of bytes.
} RillBytes;

/**
 * @brief Owned byte buffer with a trailing NUL outside the payload.
 *
 * Zero initialization creates an empty buffer. Storage belongs to this buffer
 * until transferred or cleared; embedded NUL bytes are allowed.
 */
typedef struct {
  char *data;      ///< Owned storage; may be nullptr when empty.
  size_t size;     ///< Payload bytes, including any embedded NUL.
  size_t capacity; ///< Allocated bytes.
} RillBuffer;
/**
 * @brief Append borrowed bytes, preserving the buffer on failure.
 *
 * The source must not alias buffer storage, which may move during growth.
 * A null source is valid only when size is zero.
 */
[[nodiscard]] bool rill_text_append(RillBuffer *buffer, const void *data,
                                    size_t size);
/**
 * @brief Append printf-formatted text with checked growth.
 *
 * The format and arguments must not alias buffer storage. Failure preserves
 * the payload but may change storage and capacity.
 */
[[nodiscard, gnu::format(printf, 2, 3)]]
bool rill_text_format(RillBuffer *buffer, const char *format, ...);
/**
 * @brief Append a display representation with controls and non-ASCII bytes
 * escaped.
 *
 * Each escaped byte uses an ASCII hex escape; this is not serialization.
 * The source must not alias the buffer. Failure leaves the buffer unchanged.
 */
[[nodiscard]] bool rill_text_escape(RillBuffer *buffer, RillBytes bytes);
/** @brief Release storage and reset the buffer. */
void rill_text_clear(RillBuffer *buffer);
/**
 * @brief Decode one UTF-8 scalar at a byte offset.
 *
 * Success advances offset and sets scalar. Invalid, incomplete, or exhausted
 * input returns false without changing either output.
 */
[[nodiscard]] bool rill_text_decode(const char *data, size_t size,
                                    size_t *offset, uint32_t *scalar);
/** @brief Validate UTF-8; nullptr is allowed only when size is zero. */
[[nodiscard]] bool rill_text_valid(const char *data, size_t size);
/**
 * @brief Append one Unicode scalar as UTF-8.
 *
 * Invalid scalars and allocation failure return false without changing the
 * buffer.
 */
[[nodiscard]] bool rill_text_encode(RillBuffer *buffer, uint32_t scalar);
/**
 * @brief Find the next extended grapheme boundary, or size at end of input.
 *
 * @pre Input is valid UTF-8 and offset is an existing grapheme boundary.
 */
size_t rill_text_next(const char *data, size_t size, size_t offset);
/**
 * @brief Estimate cell width for one valid UTF-8 cluster.
 *
 * Uses the current property-based width policy, with one cell for a cluster
 * without a visible base. Full emoji-sequence layout is not implemented.
 */
unsigned rill_text_width(const char *data, size_t size);
