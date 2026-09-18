/** @file
 * @brief Versioned standard-library sources embedded with the executable.
 */
#pragma once
/** @brief Borrow static module source by its std: name; nullptr if unknown. */
const char *rill_library_source(const char *name);
