# Third-party notices

This project's own code is under the MIT License (`LICENSE`). Parts of it are ports of
third-party code, which keep their own notices below.

## Mono: `System.Decimal` arithmetic

`crates/wr-sim/src/csharp/decimal.rs` is a port of Mono's `mono/metadata/decimal-ms.c` (from
Unity's fork of Mono, the runtime the game ships with), itself a port of CoreCLR's
`decimal.cpp`. Its header reads: "Copyright (c) Microsoft. All rights reserved. Licensed under
the MIT license." and "Copyright 2015 Xamarin Inc".

## .NET Framework reference source: collections

`crates/wr-sim/src/csharp/sort.rs` ports `System.Collections.Generic.ArraySortHelper<T>`, and
`crates/wr-sim/src/csharp/dict.rs` reproduces the enumeration order of
`System.Collections.Generic.Dictionary<TKey, TValue>`, as the game's mscorlib implements them
(the .NET Framework reference source, Copyright (c) Microsoft Corporation, MIT License).

## The MIT License, as it applies to the code above

Copyright (c) Microsoft Corporation. Copyright 2015 Xamarin Inc.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## Mods to be ported

ToyBox and Buff It 2 The Limit (see `README.md`) are both MIT-licensed. When their features are
ported, the port keeps their copyright notices here.
