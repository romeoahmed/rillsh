#include "bundle.h"
#include <string.h>
static const char prelude[] = {
#embed "../../stdlib/prelude.rill"
    , 0};
static const char core[] = {
#embed "../../stdlib/core.rill"
    , 0};
static const char seq[] = {
#embed "../../stdlib/seq.rill"
    , 0};
static const char text[] = {
#embed "../../stdlib/text.rill"
    , 0};
static const char process[] = {
#embed "../../stdlib/process.rill"
    , 0};
const char *rill_library_source(const char *name) {
  if (!strcmp(name, "std:prelude"))
    return prelude;
  if (!strcmp(name, "std:core"))
    return core;
  if (!strcmp(name, "std:seq"))
    return seq;
  if (!strcmp(name, "std:text"))
    return text;
  if (!strcmp(name, "std:process"))
    return process;
  return nullptr;
}
