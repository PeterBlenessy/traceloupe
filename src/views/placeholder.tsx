import { Construction } from "lucide-react";
import { EmptyView } from "@/components/view";

/** Stub view for artifact screens not yet implemented (plan M4). */
export function Placeholder({ title }: { title: string }) {
  return (
    <EmptyView
      icon={Construction}
      title={title}
      description="This view is coming soon."
    />
  );
}
