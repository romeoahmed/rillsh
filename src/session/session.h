/** @file
 * @brief CLI and canonical-input session composition.
 */
#pragma once
/**
 * @brief Run a session and return its shell exit status.
 *
 * Borrows argv and the inherited environment for this call; owns cleanup of
 * launched children, terminal state, and runtime storage before returning.
 */
int rill_session_main(int argc, char **argv, char **environment);
