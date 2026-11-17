## Windows: read selected pages with native Word rendering

Keep the existing text-reading and editing workflows. For figures, formulas, layout, or incomplete extraction, export a temporary PDF through Microsoft Word and render selected pages:

```bash
python scripts/render_pages.py "C:/path/report.docx" --pages "1,3-5" --output "C:/path/report-preview" --dpi 144
```

Pages are **one-based**; omit `--pages` only to render every page. Open the **absolute PNG paths** printed in the JSON output. The document is read-only, and the script closes its own Word instance in `finally`. Requires Windows, Microsoft Word, `pywin32`, `pypdfium2`, and `Pillow`; the implementation is shared with the sibling `pdf` skill. Use this native path for Windows visual verification; existing LibreOffice workflows remain available elsewhere.
