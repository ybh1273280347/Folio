## Windows: read selected slides with native PowerPoint rendering

Keep the existing text-reading and editing workflows. For diagrams, formulas, layout, or incomplete extraction, export selected slides through Microsoft PowerPoint:

```bash
python scripts/render_pages.py "C:/path/deck.pptx" --pages "1,3-5" --output "C:/path/deck-preview" --dpi 144
```

Slide numbers are **one-based**; omit `--pages` only to render the entire deck. Open the **absolute PNG paths** printed in the JSON output. The presentation is read-only, and the script closes its own PowerPoint instance in `finally`. Requires Windows, Microsoft PowerPoint, `pywin32`, `pypdfium2`, and `Pillow`; the implementation is shared with the sibling `pdf` skill. Use this native path for Windows visual verification; existing LibreOffice workflows remain available elsewhere.
