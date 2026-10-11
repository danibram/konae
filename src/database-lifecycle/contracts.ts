export type DatabaseMigrationPlan = {
  id: string;
  fromHash: string;
  toHash: string;
  sql: string;
  statements: string[];
  destructive: boolean;
  candidateModelSource?: string;
};

export type PlanDatabaseChangeInput = {
  acceptedSource: string;
  candidateSource: string;
};

export type ApplyDatabaseMigrationInput = DatabaseMigrationPlan & {
  recoveryModelSource: string;
};

