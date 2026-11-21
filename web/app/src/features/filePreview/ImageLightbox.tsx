// An image of a page or a markdown file opened in the zoom viewer, with its Download, over the preview. The bytes are the ones
// already loaded for the page: nothing is asked of the desktop again.
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/ui/dialog";
import { ImageViewer } from "./ImageViewer";

export interface LightboxImage {
  name: string;
  mime: string;
  chunks: Uint8Array[];
}

export function ImageLightbox({ image, onClose }: { image: LightboxImage | null; onClose: () => void }) {
  return (
    <Dialog open={image !== null} onOpenChange={(open) => !open && onClose()}>
      <DialogContent aria-describedby={undefined} data-testid="image-lightbox" className="h-[min(88dvh,60rem)] max-w-4xl grid-rows-[auto_minmax(0,1fr)] overflow-hidden">
        <DialogHeader className="min-w-0">
          <DialogTitle className="min-w-0 truncate text-title" data-testid="image-lightbox-name">
            {image?.name ?? ""}
          </DialogTitle>
        </DialogHeader>
        <div className="flex min-h-0 min-w-0 flex-col">{image ? <ImageViewer chunks={image.chunks} mime={image.mime} name={image.name} /> : null}</div>
      </DialogContent>
    </Dialog>
  );
}
