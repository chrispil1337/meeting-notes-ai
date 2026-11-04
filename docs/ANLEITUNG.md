# Meeting Notes AI – Kurzanleitung

Diese Anleitung führt Dich vom Quellcode bis zum ersten bereinigten Transkript.
Die Oberfläche ist auf Englisch oder Französisch (wie bei Soufflé). Die Begriffe unten
stehen deshalb in Englisch, in Klammern auf Deutsch.

## 1. Einmalig: Werkzeuge installieren

Im Terminal:

```bash
xcode-select --install
```

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

```bash
brew install cmake
```

(Für den letzten Befehl brauchst Du [Homebrew](https://brew.sh/).)

## 2. App bauen und starten

```bash
git clone https://github.com/chrispil1337/meeting-notes-ai.git
```

```bash
cd meeting-notes-ai && git checkout feature/meeting-notes-ai && make nightly
```

Der erste Build dauert lange (10–30 Minuten). Danach öffnet sich **Meeting Notes AI Nightly**.
Spätere Updates:

```bash
cd meeting-notes-ai && git pull && make nightly
```

## 3. Erster Start

1. Der Einrichtungs-Assistent fragt nach den macOS-Berechtigungen (Mikrofon, Systemton,
   optional Kalender). Erlauben.
2. **Sprachmodell wählen:** Für deutsche Meetings und Sprachmemos **Whisper → Large V3 Turbo**
   nehmen. Kyutai versteht nur Englisch und Französisch.
3. Das Modell wird einmalig heruntergeladen (ca. 1,6 GB).

## 4. Ollama Cloud mit DeepSeek einrichten

1. Auf [ollama.com/settings/keys](https://ollama.com/settings/keys) einen API-Schlüssel anlegen.
2. In der App: **Settings → AI** (Einstellungen → KI).
3. Schalter **Ollama Cloud** einschalten.
4. Den Schlüssel in das Feld einfügen und mit Enter bestätigen. Er wird im macOS-Schlüsselbund
   gespeichert.
5. Im Modell-Menü ein DeepSeek-Modell (oder ein anderes Cloud-Modell) wählen.

Lokales Ollama statt Cloud: Schalter aus. Dann gilt wieder die lokale Adresse
(Standard `http://localhost:11434`).

## 5. Meeting aufnehmen

Auf der Startseite **Meeting** klicken, am Ende stoppen. Die Meeting-Seite zeigt Audio,
Transkript und Zusammenfassung. **Summarize** (Zusammenfassen) erstellt die Zusammenfassung mit
dem gewählten Modell.

## 6. Sprachmemos importieren

**Vom iPhone:** In der App *Sprachmemos* das Memo antippen → **…** → **Teilen** → per AirDrop
an den Mac schicken oder **In Dateien sichern**.

**Vom Mac:** In der App *Sprachmemos* das Memo in den Finder ziehen.

Dann in Meeting Notes AI auf der Startseite **Import audio file…** (Audiodatei importieren)
klicken und eine oder mehrere Dateien wählen (m4a, mp3, wav, flac, ogg). Fortschritt und
**Cancel** (Abbrechen) stehen direkt unter dem Knopf. Jede Datei wird ein Meeting im Verlauf;
Titel ist der Dateiname, Datum das Aufnahmedatum der Datei.

Damit das Audio mitgespeichert wird (zum Anhören in der App), unter **Settings → System** bei
**Meeting audio retention** (Aufbewahrung) z. B. *Keep 30 days* wählen. Ohne diese Einstellung wird nur
das Transkript gespeichert.

## 7. Transkript bereinigen und vergleichen

Auf der Meeting-Seite über dem Transkript:

- **Original:** das unveränderte Transkript.
- **Cleaned** (Bereinigt): **Clean transcript** klicken. Die KI korrigiert Absatz für Absatz
  Rechtschreibung, Grammatik und Füllwörter. Mit **Edit** (Bearbeiten) änderst Du einen Absatz
  von Hand.
- **Comparison** (Vergleich): entfernte Wörter rot, ergänzte grün.

Hinweise:
- **original kept** (Original behalten): Die KI-Antwort war unvollständig oder unplausibel; der
  Absatz zeigt das Original.
- Wird ein Meeting später fortgesetzt, erscheint ein Hinweis, die Bereinigung erneut zu starten.

## 8. Exportieren

Auf der Meeting-Seite **Export** → Markdown oder JSON. Beide Formate enthalten Original und
bereinigte Fassung.

## Fehlerbehebung

| Meldung | Lösung |
|---|---|
| „Ollama Cloud rejected the API key“ | Schlüssel in Settings → AI neu einfügen. |
| „Ollama Cloud usage limit reached“ | Später erneut versuchen oder Tarif bei Ollama prüfen. |
| „The transcription model … is not downloaded“ | Modell in Settings → Transcription herunterladen. |
| „No speech was recognized in this file.“ | Datei abspielen und prüfen; bei deutscher Sprache Whisper verwenden. |
| „Stop the current recording before importing audio.“ | Laufende Aufnahme beenden, dann importieren. |
| Bereinigung schlägt fehl | Settings → AI: Ist ein Modell gewählt und erreichbar? |

## Datenschutz

- Transkription und Audio bleiben auf dem Mac.
- Nur bei **Ollama Cloud** wird Transkripttext (für Zusammenfassung und Bereinigung) an
  ollama.com gesendet. Mit lokalem Ollama oder Apple Intelligence verlässt nichts den Mac.
- Der API-Schlüssel liegt im macOS-Schlüsselbund unter „Meeting Notes AI“.
