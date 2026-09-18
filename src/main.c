/**
 * @file
 * @brief Pass arguments and the inherited environment to the session owner.
 */
#include "session/session.h"
extern char **environ;
int main(int argc, char **argv) {
  return rill_session_main(argc, argv, environ);
}
