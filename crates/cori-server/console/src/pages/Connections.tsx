import { useSession } from "../lib/session";
import { useTitle } from "../lib/title";
import { Accounts } from "../ui/connections";
import { PageHeader, Spinner } from "../ui/core";

// The accounts a person connected (Google): apps that read their mail, files or documents act with them, as them.

export function ConnectionsPage() {
  useTitle("Connected accounts");
  const { me, loading } = useSession();
  if (loading && !me) return <Spinner />;
  return (
    <div className="max-w-3xl">
      <PageHeader title="Connected accounts"
        description="Apps that read or write your mail, files and documents act with your own account: they reach what you reach there, nothing more. Connect it once, disconnect it any time." />
      <Accounts base="/v1/connections" mayEdit whose="person" />
    </div>
  );
}
