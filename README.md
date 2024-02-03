# termulus

A development environment for [Sesh](https://github.com/willothy/sesh)'s terminal emulator.

It's quicker to setup an egui window than a nice tui so I figured I'd try implementing it this way
since I've also never used egui before. I also would like Sesh to be fully remote so that it could
potentially be used from a GUI, and not just in the terminal.

The egui implementation is based off of streams by [sphaerophoria](https://github.com/sphaerophoria).

The parser is diverging somewhat from sphaerophoria's project, but the design
takes a lot of inspiration from it.
