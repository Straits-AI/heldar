// Standalone entry for the search module UI bundle. Default-exports the page component; react and
// friends are externals (resolved by the shell's import map), so this bundle shares the shell's React.
import { Search } from "../../pages/Search";
export default Search;
