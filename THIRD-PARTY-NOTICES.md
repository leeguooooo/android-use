# Android runner

`android-use setup` downloads the unmodified `u2.jar` 0.4.0 from
[openatx/android-uiautomator-server-jar](https://github.com/openatx/android-uiautomator-server-jar/releases/tag/0.4.0).
The runner is separate from the Rust binary and its SHA-256 is verified before use.
At startup, android-use appends its source-built Java adapter as a separate DEX.
The pinned upstream library is retained unchanged. The adapter binds only to
127.0.0.1:19008 and authenticates every control request using a per-launch token.
Its Java source, build instructions and source/DEX hashes ship in this repository;
end users do not need a Java compiler or Android SDK.
See the upstream project for its source and dependency notices.

The MIT License (MIT)
Copyright (c) 2015 xiaocong@gmail.com

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.

# Shared iphone-use core

The shared control/runtime helpers are synchronized from leeguooooo/iphone-use. Their original copyright notice and PolyForm Shield 1.0.0 terms are retained in LICENSE. shared/manifest.json records the exact source commit and hashes.
