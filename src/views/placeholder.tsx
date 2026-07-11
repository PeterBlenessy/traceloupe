/** Stub view for artifact screens not yet implemented (plan M4). */
export function Placeholder({ title }: { title: string }) {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-2 text-center">
      <h1 className="text-lg font-medium">{title}</h1>
      <p className="max-w-sm text-sm text-muted-foreground">
        This view unlocks after a backup has been imported.
      </p>
    </div>
  );
}
