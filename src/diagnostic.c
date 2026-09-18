#include "diagnostic.h"
const char *rill_diagnostic_name(RillError k) {
  switch (k) {
  case RILL_MATCH_ERROR:
    return "MatchError";
  case RILL_ARITHMETIC:
    return "ArithmeticError";
  case RILL_MISSING_FIELD:
    return "MissingField";
  case RILL_OK:
    return "Success";
  case RILL_SYNTAX:
    return "SyntaxError";
  case RILL_TYPE:
    return "TypeError";
  case RILL_LAUNCH:
    return "LaunchError";
  case RILL_PROCESS:
    return "ProcessError";
  case RILL_IO:
    return "IOError";
  case RILL_LIMIT:
    return "LimitExceeded";
  case RILL_CANCELLED:
    return "Cancelled";
  case RILL_MEMORY:
    return "OutOfMemory";
  }
  return "UnknownError";
}
