import { z } from 'zod';

export const orderIdSchema = z.string().regex(/^[0-9]{1,9}$/).transform(Number).pipe(z.number().int().positive());
export const quantitySchema = z.object({ quantity: z.number().int().min(1).max(10) }).strict();
