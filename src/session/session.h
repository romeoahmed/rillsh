/** @file
 * @brief CLI and canonical-input session composition.
 */
#pragma once
/** @brief Run one shell session, borrowing argv and the inherited environment.
 */
int rill_session_main(int argc, char **argv, char **environment);
