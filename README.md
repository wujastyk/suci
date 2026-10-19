# Sūcī — a fast BibTeX / biblatex manager

*Sūcī* (Skt. सूची, “list, index”; also “needle”) is a bibliography manager for
Linux written in Rust and GTK 4. It reads and writes the same `.bib` files as
JabRef, so you can switch between the two freely.

It was written for humanities work on South Asia: IAST diacritics, Sanskrit
alphabetical order, union-catalogue lookup for Indian and European books, and
citation styles used in the humanities.

## What it does

- **Opens big libraries instantly.** A 6,700-entry, 4.6 MB library loads in about a quarter of a second, using a fraction of JabRef's memory.
- **Never rewrites what you didn't touch.** Unchanged entries are saved byte-for-byte; edited entries are written in JabRef's layout. A `.bak` copy is kept on every save, and changes made by another program (JabRef, Dropbox) are detected before overwriting.
- **Search** that ignores case and diacritics (`sastra` finds *śāstra*), with phrases, field search, year ranges and exclusions:
  `olivelle "code of law"` · `author:pollock year:1990..2000` · `has:file -has:doi` · `type:incollection title:yoga`
- **Add from DOI or ISBN** — paste into the box at the top and press Enter (or Ctrl+Shift+V anywhere).
  DOIs use Crossref, then doi.org. ISBNs are looked up in the K10plus/GVK union catalogue, Open Library and Google Books at once, and you pick the best record. Existing DOIs/ISBNs are detected.
  The ↓ button beside an entry's DOI/ISBN fills in missing fields, showing old and new values side by side.
- **Formatted citations** for the selected entries, with italics, ready to paste into an email or a word processor: Chicago 17th author-date, New Hart's Rules 2nd (author-date and notes, with or without publisher), AMA 11th. Add any other [CSL style](https://www.zotero.org/styles) by putting the `.csl` file in `~/.config/suci/styles/`.
- **A one-page entry editor** showing the fields *you* use for that entry type, worked out from your own library, in reading order. Rarely used fields are added on demand; housekeeping fields (owner, dates) are folded away.
- **BibTeX source view** (Ctrl+U, or the *Fields | BibTeX source* switch): see and edit the raw biblatex text of an entry. Mistakes such as an unclosed brace are reported with the line to check, and nothing is changed until the text is valid.
- **Eprints / ARKs**: *Eprint* and *Eprint type* are offered on every entry; the type defaults to `ark` and is saved as soon as an eprint is entered. A pasted resolver link (`https://n2t.net/ark:/13960/…`) is stored as `ark:/13960/…`, and a button opens it in the browser.
- **Short DOIs**: the *Shorten* button beside a DOI (or right-click → *Shorten DOI*, for any number of selected entries) replaces it by its [shortDOI](https://shortdoi.org) such as `10/hbph7k`. A Preferences option stores new entries' DOIs in short form. Short DOIs can also be pasted into the *Add from DOI* box.
- **English title capitalisation** (headline style, after Chicago): right-click an entry (or a title field) → *Capitalise Titles*, or Menu → *Titles* for the whole library with a preview first. It only ever raises first letters (McGill, iPhone, al-Bīrūnī stay as written), lowers articles, conjunctions and short prepositions, keeps anything in braces or LaTeX commands (`\emph{…}`, `\dev{…}`, `\textbengali{…}`) as is, and respects lowercase Sanskrit terms and compounds (*Nyāya-vaiśeṣika*, *bhojya-*) in already-capitalised titles. Library-wide and automatic use (a Preferences option) skips entries whose `langid`/`language` is not English, titles in Devanagari, Bengali etc., and untagged titles that don't look English. A separate option capitalises only in formatted citations, without changing the data.
- **Linked PDFs**: open with Ctrl+P, attach with a button or by dropping a file on the editor. Relative links are resolved against JabRef's own PDF folder (read from JabRef's settings), the folder set in Preferences, and the `.bib` file's folder.
- **Integrity check** (F7): missing required fields (biblatex rules, crossref-aware), duplicate keys, duplicate DOIs/ISBNs, ISBN/ISSN check digits, malformed DOIs and dates, single-hyphen page ranges, HTML left over from web imports, LaTeX accents that could be Unicode, missing PDF files. Many problems have a one-click fix; “Fix all shown” applies them as a single undo step.
- **Diacritics**: convert LaTeX accents (`{\=a}`, `\d{r}`) to Unicode and back, per entry or for the whole library, leaving other LaTeX (`\emph`, `\cite`) untouched.
- **Sanskrit sort order** (menu → Sort Order): a ā i ī u ū ṛ ṝ ḷ e ai o au ṃ ḥ k kh … ś ṣ s h. The default alphabetical order puts ā right after a and ś right after s.
- **Citation keys** in your existing pattern (`wuja-2022`, or `wuja-scie`), with diacritics removed. If the key is taken, the letter after the highest one in use is added (`wuja-2022`, `wuja-2022a`, `wuja-2022c` exist → `wuja-2022d`), so a letter once used for another work is never reused. New entries get a key as soon as author and year are filled in.
- Undo/redo for every change, keyboard shortcuts throughout (F1 lists them).

Not (yet) included: JabRef groups, full-text PDF search, Word/LibreOffice integration (by design).

## Github repository

[https://github.com/wujastyk/suci](https://github.com/wujastyk/suci)

## Install on Linux Mint 22 / Ubuntu 24.04

Download the latest `suci_…_amd64.deb` from the **[Releases](https://github.com/wujastyk/suci/releases)** page and double-click it, or:

```sh
sudo apt install ./suci_0.1.0_amd64.deb
```

Then start **Sūcī** from the menu, or run `suci path/to/library.bib`.
It reopens the last library on the next start.

## Build from source

```sh
sudo apt install libgtk-4-dev build-essential
curl https://sh.rustup.rs -sSf | sh     # Rust toolchain
cargo build --release
./target/release/suci library.bib
packaging/build-deb.sh                   # makes target/deb/suci_*.deb
```

`suci --check library.bib` prints the integrity report in the terminal.

## Files

- Settings: `~/.config/suci/settings.json`
- Extra citation styles: `~/.config/suci/styles/*.csl`

## Licence

MIT (see `LICENSE`). The bundled citation styles come from the
[Citation Style Language](https://citationstyles.org/) project and are licensed
CC BY-SA 3.0. Citations are formatted with the
[hayagriva](https://github.com/typst/hayagriva) CSL engine.
