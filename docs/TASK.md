# Task language

Suggested wording, not a certification: "An authentic Human–AI co-op OS".

This is a fixed-buffer interpreter in the QEMU kernel. It is not an AI runtime. There is no heap. Maskable interrupts stay off, and COM1 is polled.

## Input

The `script` build prints `R3ATH3_SCRIPT_READY`, then reads COM1 until a line whose text is `run`. That line ends the script and is not a command. Each byte is polled at most 20,000,000 times. If the next byte does not arrive, the kernel prints `R3ATH3_SCRIPT_ERROR` with `reason=timeout` and the 1-based number of the line it was reading (`1` if nothing has arrived). `\r` is ignored. A line longer than 72 bytes is `reason=limit`.

The smoke harness waits for `R3ATH3_SCRIPT_READY`, then writes the script and a `run` line in small chunks so the 16-byte UART FIFO is not overrun.

## Limits

| Limit | Value |
| --- | --- |
| Commands | 24 |
| Line length | 72 bytes, not counting the newline |
| Variables | 8 |
| Name length | 8 |
| `say` text | 40 bytes |
| `repeat` count | 0 through 16 |
| `repeat` nesting | one level |

## Lines

`#` starts a comment outside quotes. Blank lines and comments are skipped. Other lines are commands. `lines=` on success is the number of stored commands, not comments, blanks, or `run`. Line numbers in errors count every physical line from 1, including comments and blanks, and do not count `run`.

## Commands

Integers are `i64`. Arithmetic and literal parsing use checked math. Names are ASCII letters or `_`, then letters, digits, or `_`.

| Command | Meaning |
| --- | --- |
| `set <var> <int>` | Create or replace `<var>`. |
| `add <var> <int\|var>` | Add to an existing variable. |
| `sub <var> <int\|var>` | Subtract from an existing variable. |
| `say "text"` | Print `R3ATH3_OUT text`. |
| `print <var>` | Print `R3ATH3_OUT <var>=<value>`. |
| `repeat <n>` … `end` | Run the body `n` times. `n` is a non-negative integer literal. |
| `halt` | Stop successfully. Later commands are not run. |

## Results

Success ends with `R3ATH3_SCRIPT_OK lines=<n>` and QEMU status **161** (port `0x50`). Failure prints one `R3ATH3_SCRIPT_ERROR line=<n> reason=<word>` and exits **163** (port `0x51`). Those statuses are not 33, 35, 95, 97–105, or 124–127.

| `reason` | When |
| --- | --- |
| `unknown` | The command word is not in the table. |
| `syntax` | The line does not match the command shape, including a negative `repeat`. |
| `overflow` | A literal or `add`/`sub` does not fit in `i64`. |
| `undefined` | `add`, `sub`, or `print` names a variable that was not `set`. |
| `limit` | Too many commands or variables, or a name or `say` string is too long, or a source line is too long. |
| `repeat` | `repeat` count is greater than 16. |
| `nested` | A `repeat` inside a `repeat`. |
| `unclosed` | `run` arrives while a `repeat` is open. |
| `timeout` | COM1 did not deliver the next byte in time. |
