import { completeUpTo, plainText } from "./plain";

test("colours drop out and a line redrawn in place reads as it last stood", () => {
  // apt update on a serial console: a yellow status line redrawn with \r, cleared with spaces or ESC[K.
  const apt =
    "\x1b[33m0% [Connecting to deb.debian.org]\x1b[0m\r                                 \rIgn:1 http://deb.debian.org/debian trixie InRelease\r\n" +
    "\x1b[33m0% [Working]\x1b[0m\x1b[33m0% [Working]\x1b[0m\r\x1b[KIgn:2 https://rcn-ee.com/repos/debian-trixie-armhf trixie InRelease\r\n" +
    "\x1b[33m0% [Working]\x1b[0m";
  expect(plainText(apt)).toBe("Ign:1 http://deb.debian.org/debian trixie InRelease\nIgn:2 https://rcn-ee.com/repos/debian-trixie-armhf trixie InRelease\n0% [Working]");
  // A coloured prompt keeps its trailing space, so a wait for "$ " still matches.
  expect(plainText("\x1b]0;debian@BeagleBone: ~\x07\x1b[01;32mdebian@BeagleBone\x1b[00m:\x1b[01;34m~\x1b[00m$ ")).toBe("debian@BeagleBone:~$ ");
  // Backspace, a shorter overwrite, the bell, charset picks.
  expect(plainText("abc\b\bX\x07\x1b(B\n12345\r89\n")).toBe("aXc\n89345\n");
  expect(plainText("plain\r\ntext")).toBe("plain\ntext");
});

test("an escape code still arriving waits for the next read", () => {
  expect(completeUpTo("done\x1b[3")).toBe(4);
  expect(completeUpTo("done\x1b")).toBe(4);
  expect(completeUpTo("done\x1b[33m")).toBe(9);
  expect(completeUpTo("\x1b]0;title")).toBe(0);
  expect(completeUpTo("\x1b]0;title\x07ok")).toBe(12);
  expect(completeUpTo("no escapes")).toBe(10);
});
