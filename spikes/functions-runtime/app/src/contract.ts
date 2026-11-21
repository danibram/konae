export type Order = { id: number; ownerId: string; quantity: number };
export type FunctionContext = {
  actor: { userId: string };
  data: {
    readOrder(id: number): Order | null | Promise<Order | null>;
    updateOrder(id: number, quantity: number): Order | null | Promise<Order | null>;
  };
};
export type FunctionEnvironment = { Bindings: FunctionContext };
