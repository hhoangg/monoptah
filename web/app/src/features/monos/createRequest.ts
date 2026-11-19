// A request to open the "New Mono" dialog on the Mono list, made from elsewhere (the command palette, F-75): the list
// is not mounted yet when the palette navigates to it, so the request waits here until the list takes it.
import { create } from "zustand";

export const useCreateMonoRequest = create<{ requested: boolean }>(() => ({ requested: false }));

export const requestCreateMono = () => useCreateMonoRequest.setState({ requested: true });
export const clearCreateMonoRequest = () => useCreateMonoRequest.setState({ requested: false });
