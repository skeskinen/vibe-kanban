import { useState, useCallback, useRef, useEffect } from 'react';
import {
  X,
  Image as ImageIcon,
  Upload,
  ChevronRight,
  AlertCircle,
} from 'lucide-react';
import { Button } from './button';
import { Alert, AlertDescription } from './alert';
import { cn } from '@/lib/utils';
import { imagesApi } from '@/lib/api';
import type { ImageResponse } from 'shared/types';

interface ImageUploadSectionProps {
  images: ImageResponse[];
  onImagesChange: (images: ImageResponse[]) => void;
  onUpload: (file: File) => Promise<ImageResponse>;
  onDelete?: (imageId: string) => Promise<void>;
  onImageUploaded?: (image: ImageResponse) => void; // Custom callback for upload success
  isUploading?: boolean;
  disabled?: boolean;
  readOnly?: boolean;
  collapsible?: boolean;
  defaultExpanded?: boolean;
  className?: string;
}

export function ImageUploadSection({
  images,
  onImagesChange,
  onUpload,
  onDelete,
  onImageUploaded,
  isUploading = false,
  disabled = false,
  readOnly = false,
  collapsible = true,
  defaultExpanded = false,
  className,
}: ImageUploadSectionProps) {
  const [isExpanded, setIsExpanded] = useState(
    defaultExpanded || images.length > 0
  );
  const [isDragging, setIsDragging] = useState(false);
  const [uploadingFiles, setUploadingFiles] = useState<Set<string>>(new Set());
  const [errorMessage, setErrorMessage] = useState<string | null>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);

  const handleFileSelect = useCallback(
    async (files: FileList | File[] | null) => {
      if (!files || disabled) return;

      const fileArray = Array.isArray(files) ? files : Array.from(files);

      setErrorMessage(null);

      const MAX_SIZE = 20 * 1024 * 1024; // 20MB
      const VALID_TYPES = [
        'image/png',
        'image/jpeg',
        'image/jpg',
        'image/gif',
        'image/webp',
        'image/bmp',
        'image/svg+xml',
      ];

      const invalidFiles: string[] = [];
      const oversizedFiles: string[] = [];
      const validFiles: File[] = [];

      fileArray.forEach((file) => {
        if (!VALID_TYPES.includes(file.type.toLowerCase())) {
          invalidFiles.push(file.name);
          return;
        }

        if (file.size > MAX_SIZE) {
          oversizedFiles.push(
            `${file.name} (${(file.size / 1048576).toFixed(1)} MB)`
          );
          return;
        }

        validFiles.push(file);
      });

      if (invalidFiles.length > 0 || oversizedFiles.length > 0) {
        const errors: string[] = [];
        if (invalidFiles.length > 0) {
          errors.push(`Unsupported file type: ${invalidFiles.join(', ')}`);
        }
        if (oversizedFiles.length > 0) {
          errors.push(
            `Files too large (max 20 MB): ${oversizedFiles.join(', ')}`
          );
        }
        setErrorMessage(errors.join('. '));
      }

      for (const file of validFiles) {
        const tempId = `uploading-${Date.now()}-${file.name}`;
        setUploadingFiles((prev) => new Set(prev).add(tempId));

        try {
          const uploadedImage = await onUpload(file);

          // Call custom upload callback if provided, otherwise use default behavior
          if (onImageUploaded) {
            onImageUploaded(uploadedImage);
          } else {
            onImagesChange([...images, uploadedImage]);
          }

          setErrorMessage(null);
        } catch (error: any) {
          console.error('Failed to upload image:', error);
          const message =
            error.message || 'Failed to upload image. Please try again.';
          setErrorMessage(message);
        } finally {
          setUploadingFiles((prev) => {
            const next = new Set(prev);
            next.delete(tempId);
            return next;
          });
        }
      }
    },
    [images, onImagesChange, onUpload, disabled]
  );

  const handleDrop = useCallback(
    (e: React.DragEvent) => {
      e.preventDefault();
      setIsDragging(false);
      handleFileSelect(e.dataTransfer.files);
    },
    [handleFileSelect]
  );

  const handleDragOver = useCallback((e: React.DragEvent) => {
    e.preventDefault();
    setIsDragging(true);
  }, []);

  const handleDragLeave = useCallback((e: React.DragEvent) => {
    e.preventDefault();
    setIsDragging(false);
  }, []);

  const handleRemoveImage = useCallback(
    async (imageId: string) => {
      if (onDelete) {
        try {
          await onDelete(imageId);
        } catch (error) {
          console.error('Failed to delete image:', error);
        }
      }
      onImagesChange(images.filter((img) => img.id !== imageId));
    },
    [images, onImagesChange, onDelete]
  );

  const formatFileSize = (bytes: bigint) => {
    const kb = Number(bytes) / 1024;
    if (kb < 1024) {
      return `${kb.toFixed(1)} KB`;
    }
    return `${(kb / 1024).toFixed(1)} MB`;
  };

  useEffect(() => {
    if (readOnly) return;

    const handlePasteEvent = (event: ClipboardEvent) => {
      if (disabled || !event.clipboardData) {
        return;
      }

      const items = Array.from(event.clipboardData.items).filter((item) =>
        item.type.startsWith('image/')
      );
      const filesFromItems = items
        .map((item) => item.getAsFile())
        .filter((file): file is File => Boolean(file));
      const filesFromClipboard = Array.from(event.clipboardData.files ?? []).filter(
        (file) => file.type.startsWith('image/')
      );

      const files = filesFromItems.length > 0 ? filesFromItems : filesFromClipboard;

      if (files.length === 0) {
        return;
      }

      event.preventDefault();
      void handleFileSelect(files);
    };

    window.addEventListener('paste', handlePasteEvent);

    return () => {
      window.removeEventListener('paste', handlePasteEvent);
    };
  }, [disabled, handleFileSelect, readOnly]);

  const content = (
    <div className={cn('space-y-3', className)}>
      {/* Error message */}
      {errorMessage && (
        <Alert variant="destructive">
          <AlertCircle className="h-4 w-4" />
          <AlertDescription>{errorMessage}</AlertDescription>
        </Alert>
      )}

      {/* Read-only message */}
      {readOnly && images.length === 0 && (
        <p className="text-sm text-muted-foreground">No images attached</p>
      )}

      {/* Drop zone - only show when not read-only */}
      {!readOnly && (
        <div
          className={cn(
            'border-2 border-dashed rounded-lg p-6 text-center transition-colors',
            isDragging
              ? 'border-primary bg-primary/5'
              : 'border-muted-foreground/25 hover:border-muted-foreground/50',
            disabled && 'opacity-50 cursor-not-allowed'
          )}
          onDrop={handleDrop}
          onDragOver={handleDragOver}
          onDragLeave={handleDragLeave}
        >
          <Upload className="h-8 w-8 mx-auto mb-3 text-muted-foreground" />
          <p className="text-sm text-muted-foreground mb-1">
            Drag, paste, or click to add images
          </p>
          <Button
            variant="secondary"
            size="sm"
            onClick={() => fileInputRef.current?.click()}
            disabled={disabled || isUploading}
          >
            Select Images
          </Button>
          <input
            ref={fileInputRef}
            type="file"
            accept="image/*"
            multiple
            className="hidden"
            onChange={(e) => handleFileSelect(e.target.files)}
            disabled={disabled}
          />
        </div>
      )}

      {/* Image previews */}
      {images.length > 0 && (
        <div className="grid grid-cols-2 gap-2">
          {images.map((image) => (
            <div
              key={image.id}
              className="relative group border rounded-lg p-2 bg-background"
            >
              <div className="flex items-center gap-2">
                <img
                  src={imagesApi.getImageUrl(image.id)}
                  alt={image.original_name}
                  className="h-16 w-16 object-cover rounded"
                />
                <div className="flex-1 min-w-0">
                  <p className="text-xs font-medium truncate">
                    {image.original_name}
                  </p>
                  <p className="text-xs text-muted-foreground">
                    {formatFileSize(image.size_bytes)}
                  </p>
                </div>
              </div>
              {!disabled && !readOnly && (
                <Button
                  variant="ghost"
                  size="icon"
                  className="absolute top-1 right-1 h-6 w-6 opacity-0 group-hover:opacity-100 transition-opacity"
                  onClick={() => handleRemoveImage(image.id)}
                >
                  <X className="h-3 w-3" />
                </Button>
              )}
            </div>
          ))}
        </div>
      )}

      {/* Uploading indicators */}
      {uploadingFiles.size > 0 && (
        <div className="space-y-1">
          {Array.from(uploadingFiles).map((tempId) => (
            <div
              key={tempId}
              className="flex items-center gap-2 text-xs text-muted-foreground"
            >
              <div className="h-3 w-3 border-2 border-primary border-t-transparent rounded-full animate-spin" />
              <span>Uploading...</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );

  if (!collapsible) {
    return content;
  }

  return (
    <div className="space-y-2">
      <button
        type="button"
        onClick={() => setIsExpanded(!isExpanded)}
        className="flex items-center gap-2 text-sm text-muted-foreground hover:text-foreground transition-colors"
      >
        <ChevronRight
          className={cn(
            'h-3 w-3 transition-transform',
            isExpanded && 'rotate-90'
          )}
        />
        <ImageIcon className="h-4 w-4" />
        <span>Images {images.length > 0 && `(${images.length})`}</span>
      </button>
      {isExpanded && content}
    </div>
  );
}
