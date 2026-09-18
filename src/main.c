/**
 * @file
 * @brief Executable entry point.
 *
 * Pass argv and the inherited environment to the session, which owns startup,
 * evaluation, and shutdown.
 */
#include "session/session.h"
extern char **environ;
int main(int argc, char **argv) {
  return rill_session_main(argc, argv, environ);
}
