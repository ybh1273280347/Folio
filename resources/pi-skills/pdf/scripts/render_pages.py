"""Render selected PDF, Word, or PowerPoint pages with Windows-native Office layout."""
import argparse
import json
from pathlib import Path
import tempfile

import pypdfium2 as pdfium


def page_numbers(selection: str | None, count: int) -> list[int]:
    """Return unique one-based pages, rejecting invalid user ranges before exporting."""
    if selection is None:
        return list(range(1, count + 1))
    pages = set()
    for token in selection.split(","):
        bounds = token.strip().split("-")
        start, end = int(bounds[0]), int(bounds[-1])
        if len(bounds) > 2 or start < 1 or end < start or end > count:
            raise ValueError(f"页码必须位于 1–{count}，支持 1,3-5")
        pages.update(range(start, end + 1))
    return sorted(pages)


def render_pdf(source: Path, selection: str | None, output: Path, dpi: int) -> list[dict]:
    """Open only selected pages for rasterization; PDFs retain their original numbering."""
    images = []
    with pdfium.PdfDocument(str(source)) as document:
        for number in page_numbers(selection, len(document)):
            destination = output / f"page-{number:04d}.png"
            page = document[number - 1]
            try:
                bitmap = page.render(scale=dpi / 72)
                try:
                    bitmap.to_pil().save(destination)
                finally:
                    bitmap.close()
            finally:
                page.close()
            images.append({"page": number, "path": str(destination.resolve())})
    return images


def render_office(source: Path, selection: str | None, output: Path, dpi: int) -> list[dict]:
    """Own a separate COM application and close it even when export or rasterization fails."""
    import pythoncom
    import win32com.client

    pythoncom.CoInitialize()
    application = None
    document = None
    try:
        if source.suffix.lower() in {".ppt", ".pptx", ".potx"}:
            application = win32com.client.DispatchEx("PowerPoint.Application")
            application.AutomationSecurity = 3
            document = application.Presentations.Open(str(source), ReadOnly=-1, Untitled=0, WithWindow=0)
            width = round(document.PageSetup.SlideWidth * dpi / 72)
            height = round(document.PageSetup.SlideHeight * dpi / 72)
            images = []
            for number in page_numbers(selection, document.Slides.Count):
                destination = output / f"page-{number:04d}.png"
                document.Slides.Item(number).Export(str(destination), "PNG", width, height)
                images.append({"page": number, "path": str(destination.resolve())})
            return images
        application = win32com.client.DispatchEx("Word.Application")
        application.Visible = False
        application.DisplayAlerts = 0
        application.AutomationSecurity = 3
        document = application.Documents.Open(str(source), ReadOnly=True, AddToRecentFiles=False)
        with tempfile.TemporaryDirectory(prefix="folio-office-render-") as temporary:
            pdf_path = Path(temporary) / "document.pdf"
            document.ExportAsFixedFormat(str(pdf_path), 17)
            return render_pdf(pdf_path, selection, output, dpi)
    finally:
        try:
            if document is not None:
                if source.suffix.lower() in {".doc", ".docx", ".dotx"}:
                    document.Close(False)
                else:
                    document.Close()
        finally:
            try:
                if application is not None:
                    application.Quit()
            finally:
                document = None
                application = None
                pythoncom.CoUninitialize()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", type=Path)
    parser.add_argument("--pages", help="One-based pages, e.g. 1,3-5; omitted means all pages")
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--dpi", default=144, type=int)
    args = parser.parse_args()
    source = args.input.resolve(strict=True)
    if args.dpi <= 0:
        raise ValueError("DPI 必须大于零")
    if source.suffix.lower() not in {".pdf", ".doc", ".docx", ".dotx", ".ppt", ".pptx", ".potx"}:
        raise ValueError("仅支持 PDF、Word 和 PowerPoint 文件")
    args.output.mkdir(parents=True, exist_ok=True)
    renderer = render_pdf if source.suffix.lower() == ".pdf" else render_office
    images = renderer(source, args.pages, args.output.resolve(), args.dpi)
    print(json.dumps({"source": str(source), "images": images}))


if __name__ == "__main__":
    main()
