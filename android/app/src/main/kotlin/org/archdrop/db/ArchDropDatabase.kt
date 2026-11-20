package org.archdrop.db

import android.content.Context
import androidx.room.Database
import androidx.room.Room
import androidx.room.RoomDatabase

@Database(
    entities = [
        EventEntity::class,
        VectorClockEntity::class,
        OutboxEntity::class,
        PeerEntity::class
    ],
    version = 2,
    exportSchema = false
)
abstract class ArchDropDatabase : RoomDatabase() {
    abstract fun eventDao(): EventDao
    abstract fun vectorClockDao(): VectorClockDao
    abstract fun outboxDao(): OutboxDao
    abstract fun peerDao(): PeerDao

    companion object {
        @Volatile
        private var INSTANCE: ArchDropDatabase? = null

        fun getInstance(context: Context): ArchDropDatabase {
            return INSTANCE ?: synchronized(this) {
                val instance = Room.databaseBuilder(
                    context.applicationContext,
                    ArchDropDatabase::class.java,
                    "archdrop_ledger.db"
                )
                .fallbackToDestructiveMigration()
                .build()
                INSTANCE = instance
                instance
            }
        }
    }
}
